use std::fmt::{self, Write};

use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, Postgres, Row, Transaction};

pub const CURRENT_VERSION: i64 = 4;

const BOOTSTRAP_SQL: &str = "CREATE SCHEMA rustodon; \
CREATE TABLE rustodon.schema_migrations ( \
  version bigint NOT NULL, checksum bytea NOT NULL, owner name NOT NULL, \
  applied_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(), \
  CONSTRAINT schema_migrations_pkey PRIMARY KEY (version), \
  CONSTRAINT schema_migrations_version_check CHECK (version > 0), \
  CONSTRAINT schema_migrations_checksum_check CHECK (octet_length(checksum) = 32)); \
REVOKE ALL ON SCHEMA rustodon FROM PUBLIC; \
REVOKE ALL ON TABLE rustodon.schema_migrations FROM PUBLIC";

const MIGRATION_1_SQL: &str = include_str!("../migrations/rustodon/0001_operational.sql");
const MIGRATION_2_SQL: &str = include_str!("../migrations/rustodon/0002_rate_limit_windows.sql");
const MIGRATION_3_SQL: &str = include_str!("../migrations/rustodon/0003_remote_fetch_leases.sql");
const MIGRATION_4_SQL: &str = include_str!("../migrations/rustodon/0004_stream_outbox_indexes.sql");
const TABLES: &[&str] = &[
    "domain_health",
    "durable_jobs",
    "heartbeats",
    "idempotency_keys",
    "ordering_markers",
    "outbox_events",
    "rate_limit_windows",
    "remote_fetch_leases",
    "schema_migrations",
];
const SEQUENCES: &[&str] = &["durable_jobs_id_seq", "outbox_events_id_seq"];
const INDEXES: &[&str] = &[
    "domain_health_pkey",
    "domain_health_retry_idx",
    "durable_jobs_claim_idx",
    "durable_jobs_dead_idx",
    "durable_jobs_lease_idx",
    "durable_jobs_logical_key_idx",
    "durable_jobs_pkey",
    "heartbeats_heartbeat_idx",
    "heartbeats_pkey",
    "idempotency_keys_expires_idx",
    "idempotency_keys_pkey",
    "ordering_markers_expires_idx",
    "ordering_markers_pkey",
    "outbox_events_logical_key_idx",
    "outbox_events_pending_idx",
    "outbox_events_pkey",
    "outbox_events_stream_created_at_idx",
    "outbox_events_stream_id_idx",
    "rate_limit_windows_expires_idx",
    "rate_limit_windows_pkey",
    "remote_fetch_leases_expires_idx",
    "remote_fetch_leases_pkey",
    "schema_migrations_pkey",
];
const EXPECTED_CATALOG_SHA256: [u8; 32] = [
    0xf2, 0xc4, 0xc1, 0x37, 0xfd, 0x98, 0xa6, 0x4e, 0x61, 0xa3, 0xa3, 0x79, 0x5c, 0x4d, 0x9f, 0xe1,
    0x1c, 0x79, 0xb9, 0x74, 0x35, 0x7a, 0x5f, 0xab, 0xe1, 0x4a, 0x4c, 0xb3, 0xe1, 0x0d, 0xae, 0x6e,
];
const CATALOG_QUERY: &str = r#"
WITH schema_info AS (
  SELECT n.oid, n.nspowner, n.nspacl
    FROM pg_catalog.pg_namespace n WHERE n.nspname = 'rustodon'
), expected_owner AS (
  SELECT r.oid FROM pg_catalog.pg_roles r
   WHERE r.rolname = pg_catalog.current_setting('rustodon.expected_owner')
), runtime_role AS (
   SELECT r.oid FROM pg_catalog.pg_roles r CROSS JOIN expected_owner owner
    WHERE r.rolname = pg_catalog.current_setting('rustodon.runtime_role', true)
      AND r.oid <> owner.oid
), writer_role AS (
   SELECT r.oid FROM pg_catalog.pg_roles r
    WHERE r.rolname = pg_catalog.current_setting('rustodon.writer_role', true)
), acl_entries AS (
  SELECT 'schema'::text AS kind, s.oid::text AS object_key,
         acl.grantee, acl.grantor, acl.privilege_type, acl.is_grantable
    FROM schema_info s
    CROSS JOIN LATERAL pg_catalog.aclexplode(
      COALESCE(s.nspacl, pg_catalog.acldefault('n', s.nspowner))) acl
  UNION ALL
  SELECT 'relation', c.oid::text, acl.grantee, acl.grantor,
         acl.privilege_type, acl.is_grantable
    FROM pg_catalog.pg_class c CROSS JOIN schema_info s
    CROSS JOIN LATERAL pg_catalog.aclexplode(COALESCE(
      c.relacl,
      pg_catalog.acldefault(
        CASE WHEN c.relkind = 'S' THEN 's'::"char" ELSE 'r'::"char" END,
        c.relowner))) acl
   WHERE c.relnamespace = s.oid AND c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')
  UNION ALL
  SELECT 'column', c.oid::text || ':' || a.attnum::text,
         acl.grantee, acl.grantor, acl.privilege_type, acl.is_grantable
    FROM pg_catalog.pg_class c CROSS JOIN schema_info s
    JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid
    CROSS JOIN LATERAL pg_catalog.aclexplode(a.attacl) acl
   WHERE c.relnamespace = s.oid AND a.attnum > 0 AND NOT a.attisdropped
  UNION ALL
  SELECT 'function', p.oid::text, acl.grantee, acl.grantor,
         acl.privilege_type, acl.is_grantable
    FROM pg_catalog.pg_proc p CROSS JOIN schema_info s
    CROSS JOIN LATERAL pg_catalog.aclexplode(
      COALESCE(p.proacl, pg_catalog.acldefault('f', p.proowner))) acl
   WHERE p.pronamespace = s.oid
  UNION ALL
  SELECT 'type', t.oid::text, acl.grantee, acl.grantor,
         acl.privilege_type, acl.is_grantable
    FROM pg_catalog.pg_type t CROSS JOIN schema_info s
    CROSS JOIN LATERAL pg_catalog.aclexplode(
      COALESCE(t.typacl, pg_catalog.acldefault('T', t.typowner))) acl
   WHERE t.typnamespace = s.oid
  UNION ALL
  SELECT 'default_acl', d.oid::text, acl.grantee, acl.grantor,
         acl.privilege_type, acl.is_grantable
    FROM pg_catalog.pg_default_acl d CROSS JOIN schema_info s
    CROSS JOIN LATERAL pg_catalog.aclexplode(d.defaclacl) acl
   WHERE d.defaclnamespace = s.oid
), normalized_acl_entries AS (
  SELECT kind, object_key,
         CASE WHEN acl.grantee = 0 THEN 'PUBLIC'
              WHEN acl.grantee = role.oid THEN 'CURRENT_USER'
              ELSE 'ROLE:' || pg_catalog.pg_get_userbyid(acl.grantee) END AS grantee,
         CASE WHEN acl.grantor = role.oid THEN 'CURRENT_USER'
              ELSE 'ROLE:' || pg_catalog.pg_get_userbyid(acl.grantor) END AS grantor,
         acl.privilege_type, acl.is_grantable
    FROM acl_entries acl CROSS JOIN expected_owner role
    WHERE NOT EXISTS (SELECT 1 FROM runtime_role runtime WHERE runtime.oid = acl.grantee)
      AND NOT EXISTS (SELECT 1 FROM writer_role writer WHERE writer.oid = acl.grantee)
), acl_sets AS (
  SELECT kind, object_key,
         pg_catalog.jsonb_agg(
           pg_catalog.jsonb_build_object(
             'grantee', grantee, 'grantor', grantor,
             'privilege', privilege_type, 'grantable', is_grantable)
           ORDER BY grantee COLLATE "C", grantor COLLATE "C",
                    privilege_type COLLATE "C", is_grantable) AS privileges
    FROM normalized_acl_entries GROUP BY kind, object_key
), entries AS (
  SELECT 'schema' AS kind, 'rustodon' AS name,
         pg_catalog.jsonb_build_object(
           'owner_is_current_user', s.nspowner = role.oid,
           'acl', COALESCE(acl.privileges, '[]'::jsonb))::text AS definition
    FROM schema_info s CROSS JOIN expected_owner role
    LEFT JOIN acl_sets acl ON acl.kind = 'schema' AND acl.object_key = s.oid::text
  UNION ALL
  SELECT 'relation' AS kind, c.relname::text AS name,
         pg_catalog.jsonb_build_object(
           'kind', c.relkind, 'persistence', c.relpersistence,
           'replica_identity', c.relreplident, 'rls', c.relrowsecurity,
           'force_rls', c.relforcerowsecurity, 'is_partition', c.relispartition,
           'partition_bound', pg_catalog.pg_get_expr(c.relpartbound, c.oid, true),
           'access_method', am.amname, 'tablespace', tablespace.spcname,
           'populated', c.relispopulated, 'options', c.reloptions,
           'toast_access_method', toast_am.amname,
           'toast_tablespace', toast_tablespace.spcname, 'toast_options', toast.reloptions,
           'owner_is_current_user', c.relowner = role.oid,
           'acl', COALESCE(acl.privileges, '[]'::jsonb))::text AS definition
    FROM pg_catalog.pg_class c CROSS JOIN schema_info s CROSS JOIN expected_owner role
    LEFT JOIN acl_sets acl ON acl.kind = 'relation' AND acl.object_key = c.oid::text
    LEFT JOIN pg_catalog.pg_am am ON am.oid = c.relam
    LEFT JOIN pg_catalog.pg_tablespace tablespace ON tablespace.oid = c.reltablespace
    LEFT JOIN pg_catalog.pg_class toast ON toast.oid = c.reltoastrelid
    LEFT JOIN pg_catalog.pg_am toast_am ON toast_am.oid = toast.relam
    LEFT JOIN pg_catalog.pg_tablespace toast_tablespace ON toast_tablespace.oid = toast.reltablespace
   WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'column', c.relname || '.' || a.attname,
         pg_catalog.jsonb_build_object(
            'position', a.attnum, 'type', pg_catalog.format_type(a.atttypid, a.atttypmod),
            'not_null', a.attnotnull, 'default', pg_catalog.pg_get_expr(d.adbin, d.adrelid),
            'identity', a.attidentity, 'generated', a.attgenerated,
            'compression', a.attcompression, 'storage', a.attstorage,
            'statistics', a.attstattarget, 'options', a.attoptions,
            'collation', CASE WHEN a.attcollation = 0 THEN NULL
              ELSE con.nspname || '.' || co.collname END,
            'acl', COALESCE(acl.privileges, '[]'::jsonb))::text
     FROM pg_catalog.pg_class c CROSS JOIN schema_info s
     JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
     LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum
     LEFT JOIN pg_catalog.pg_collation co ON co.oid = a.attcollation
     LEFT JOIN pg_catalog.pg_namespace con ON con.oid = co.collnamespace
     LEFT JOIN acl_sets acl ON acl.kind = 'column'
       AND acl.object_key = c.oid::text || ':' || a.attnum::text
    WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'constraint',
         COALESCE(c.relname || '.', t.typname || '.') || con.conname,
         pg_catalog.jsonb_build_object(
           'type', con.contype, 'deferrable', con.condeferrable,
           'initially_deferred', con.condeferred, 'validated', con.convalidated,
           'no_inherit', con.connoinherit,
           'definition', pg_catalog.pg_get_constraintdef(con.oid, true))::text
    FROM pg_catalog.pg_constraint con CROSS JOIN schema_info s
    LEFT JOIN pg_catalog.pg_class c ON c.oid = con.conrelid
    LEFT JOIN pg_catalog.pg_type t ON t.oid = con.contypid
   WHERE con.connamespace = s.oid
  UNION ALL
  SELECT 'index', t.relname || '.' || i.relname,
         pg_catalog.jsonb_build_object(
           'unique', x.indisunique, 'primary', x.indisprimary,
           'exclusion', x.indisexclusion, 'immediate', x.indimmediate,
           'clustered', x.indisclustered, 'valid', x.indisvalid,
           'ready', x.indisready, 'live', x.indislive,
           'definition', pg_catalog.pg_get_indexdef(i.oid))::text
    FROM pg_catalog.pg_index x CROSS JOIN schema_info s
    JOIN pg_catalog.pg_class i ON i.oid = x.indexrelid
    JOIN pg_catalog.pg_class t ON t.oid = x.indrelid
   WHERE i.relnamespace = s.oid
  UNION ALL
  SELECT 'sequence', c.relname,
         pg_catalog.jsonb_build_object(
            'type', pg_catalog.format_type(seq.seqtypid, NULL), 'start', seq.seqstart,
            'increment', seq.seqincrement, 'min', seq.seqmin, 'max', seq.seqmax,
            'cache', seq.seqcache, 'cycle', seq.seqcycle,
            'owned_by', owner_ns.nspname || '.' || owner.relname || '.' || attribute.attname)::text
     FROM pg_catalog.pg_sequence seq CROSS JOIN schema_info s
     JOIN pg_catalog.pg_class c ON c.oid = seq.seqrelid
     LEFT JOIN pg_catalog.pg_depend dep ON dep.classid = 'pg_class'::regclass
       AND dep.objid = c.oid AND dep.deptype IN ('a', 'i')
     LEFT JOIN pg_catalog.pg_class owner ON owner.oid = dep.refobjid
     LEFT JOIN pg_catalog.pg_namespace owner_ns ON owner_ns.oid = owner.relnamespace
     LEFT JOIN pg_catalog.pg_attribute attribute
       ON attribute.attrelid = dep.refobjid AND attribute.attnum = dep.refobjsubid
    WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'type', t.typname,
         pg_catalog.jsonb_build_object(
           'type', t.typtype, 'category', t.typcategory, 'preferred', t.typispreferred,
           'not_null', t.typnotnull,
           'base_type', CASE WHEN t.typbasetype = 0 THEN NULL
             ELSE pg_catalog.format_type(t.typbasetype, t.typtypmod) END,
           'dimensions', t.typndims, 'default', t.typdefault,
           'collation', CASE WHEN t.typcollation = 0 THEN NULL
             ELSE con.nspname || '.' || co.collname END,
           'owner_is_current_user', t.typowner = role.oid,
           'acl', COALESCE(acl.privileges, '[]'::jsonb))::text
     FROM pg_catalog.pg_type t CROSS JOIN schema_info s
     CROSS JOIN expected_owner role
     LEFT JOIN pg_catalog.pg_collation co ON co.oid = t.typcollation
     LEFT JOIN pg_catalog.pg_namespace con ON con.oid = co.collnamespace
     LEFT JOIN acl_sets acl ON acl.kind = 'type' AND acl.object_key = t.oid::text
    WHERE t.typnamespace = s.oid
  UNION ALL
  SELECT 'enum_label', t.typname || '.' || e.enumlabel,
         pg_catalog.jsonb_build_object('order', e.enumsortorder)::text
    FROM pg_catalog.pg_enum e JOIN pg_catalog.pg_type t ON t.oid = e.enumtypid
    CROSS JOIN schema_info s WHERE t.typnamespace = s.oid
  UNION ALL
  SELECT 'range', t.typname,
         pg_catalog.jsonb_build_object(
           'subtype', pg_catalog.format_type(r.rngsubtype, NULL),
           'collation', CASE WHEN r.rngcollation = 0 THEN NULL
             ELSE con.nspname || '.' || co.collname END,
           'canonical', r.rngcanonical::regprocedure::text,
           'subdiff', r.rngsubdiff::regprocedure::text,
           'multirange', pg_catalog.format_type(r.rngmultitypid, NULL))::text
    FROM pg_catalog.pg_range r JOIN pg_catalog.pg_type t ON t.oid = r.rngtypid
    CROSS JOIN schema_info s
    LEFT JOIN pg_catalog.pg_collation co ON co.oid = r.rngcollation
    LEFT JOIN pg_catalog.pg_namespace con ON con.oid = co.collnamespace
   WHERE t.typnamespace = s.oid
  UNION ALL
  SELECT 'function', p.proname || '(' || pg_catalog.pg_get_function_identity_arguments(p.oid) || ')',
         pg_catalog.jsonb_build_object(
           'result', pg_catalog.pg_get_function_result(p.oid), 'kind', p.prokind,
           'language', language.lanname, 'volatility', p.provolatile,
           'parallel', p.proparallel, 'security_definer', p.prosecdef,
           'leakproof', p.proleakproof, 'strict', p.proisstrict,
           'returns_set', p.proretset, 'support', p.prosupport::regprocedure::text,
           'cost', p.procost, 'rows', p.prorows, 'config', p.proconfig,
           'body', p.prosrc, 'binary', p.probin,
           'owner_is_current_user', p.proowner = role.oid,
           'acl', COALESCE(acl.privileges, '[]'::jsonb))::text
    FROM pg_catalog.pg_proc p CROSS JOIN schema_info s CROSS JOIN expected_owner role
    JOIN pg_catalog.pg_language language ON language.oid = p.prolang
    LEFT JOIN acl_sets acl ON acl.kind = 'function' AND acl.object_key = p.oid::text
   WHERE p.pronamespace = s.oid
  UNION ALL
  SELECT 'aggregate', p.proname || '(' || pg_catalog.pg_get_function_identity_arguments(p.oid) || ')',
         pg_catalog.jsonb_build_object(
           'kind', a.aggkind, 'arguments', a.aggnumdirectargs,
           'transition', a.aggtransfn::regprocedure::text,
           'final', a.aggfinalfn::regprocedure::text,
           'combine', a.aggcombinefn::regprocedure::text,
           'serialize', a.aggserialfn::regprocedure::text,
           'deserialize', a.aggdeserialfn::regprocedure::text,
           'state_type', pg_catalog.format_type(a.aggtranstype, NULL),
           'initial', a.agginitval)::text
    FROM pg_catalog.pg_aggregate a JOIN pg_catalog.pg_proc p ON p.oid = a.aggfnoid
    CROSS JOIN schema_info s WHERE p.pronamespace = s.oid
  UNION ALL
  SELECT 'rule', c.relname || '.' || r.rulename,
         pg_catalog.pg_get_ruledef(r.oid, true)
    FROM pg_catalog.pg_rewrite r JOIN pg_catalog.pg_class c ON c.oid = r.ev_class
    CROSS JOIN schema_info s WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'trigger', c.relname || '.' || t.tgname,
         pg_catalog.jsonb_build_object(
           'internal', t.tgisinternal, 'enabled', t.tgenabled,
           'definition', pg_catalog.pg_get_triggerdef(t.oid, true))::text
    FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
    CROSS JOIN schema_info s WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'policy', c.relname || '.' || p.polname,
         pg_catalog.jsonb_build_object(
           'permissive', p.polpermissive, 'command', p.polcmd,
           'roles', (SELECT pg_catalog.jsonb_agg(
               CASE WHEN role_oid = 0 THEN 'PUBLIC'
                    WHEN role_oid = role.oid THEN 'CURRENT_USER'
                    ELSE 'ROLE:' || pg_catalog.pg_get_userbyid(role_oid) END
               ORDER BY role_oid)
             FROM unnest(p.polroles) role_oid),
           'using', pg_catalog.pg_get_expr(p.polqual, p.polrelid, true),
           'check', pg_catalog.pg_get_expr(p.polwithcheck, p.polrelid, true))::text
    FROM pg_catalog.pg_policy p JOIN pg_catalog.pg_class c ON c.oid = p.polrelid
    CROSS JOIN schema_info s CROSS JOIN expected_owner role WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'inheritance', child_ns.nspname || '.' || child.relname || '->' ||
                        parent_ns.nspname || '.' || parent.relname,
         pg_catalog.jsonb_build_object('sequence', i.inhseqno, 'detached', i.inhdetachpending)::text
    FROM pg_catalog.pg_inherits i
    JOIN pg_catalog.pg_class child ON child.oid = i.inhrelid
    JOIN pg_catalog.pg_namespace child_ns ON child_ns.oid = child.relnamespace
    JOIN pg_catalog.pg_class parent ON parent.oid = i.inhparent
    JOIN pg_catalog.pg_namespace parent_ns ON parent_ns.oid = parent.relnamespace
    CROSS JOIN schema_info s
   WHERE child.relnamespace = s.oid OR parent.relnamespace = s.oid
  UNION ALL
  SELECT 'default_acl', d.defaclobjtype::text || ':' ||
         CASE WHEN d.defaclrole = role.oid THEN 'CURRENT_USER'
              ELSE 'ROLE:' || pg_catalog.pg_get_userbyid(d.defaclrole) END,
         pg_catalog.jsonb_build_object(
           'owner_is_current_user', d.defaclrole = role.oid,
           'acl', COALESCE(acl.privileges, '[]'::jsonb))::text
    FROM pg_catalog.pg_default_acl d CROSS JOIN schema_info s CROSS JOIN expected_owner role
    LEFT JOIN acl_sets acl ON acl.kind = 'default_acl' AND acl.object_key = d.oid::text
   WHERE d.defaclnamespace = s.oid
  UNION ALL
  SELECT 'collation', c.collname,
         pg_catalog.jsonb_build_object(
           'provider', c.collprovider, 'deterministic', c.collisdeterministic,
           'encoding', c.collencoding, 'collate', c.collcollate,
           'ctype', c.collctype, 'version', c.collversion,
           'owner_is_current_user', c.collowner = role.oid)::text
    FROM pg_catalog.pg_collation c CROSS JOIN schema_info s CROSS JOIN expected_owner role
   WHERE c.collnamespace = s.oid
  UNION ALL
  SELECT 'conversion', c.conname,
         pg_catalog.jsonb_build_object(
           'source', c.conforencoding, 'destination', c.contoencoding,
           'function', c.conproc::regprocedure::text, 'default', c.condefault,
           'owner_is_current_user', c.conowner = role.oid)::text
    FROM pg_catalog.pg_conversion c CROSS JOIN schema_info s CROSS JOIN expected_owner role
   WHERE c.connamespace = s.oid
  UNION ALL
  SELECT 'operator', o.oprname || '(' || o.oprleft::regtype::text || ',' || o.oprright::regtype::text || ')',
         pg_catalog.jsonb_build_object(
           'kind', o.oprkind, 'can_merge', o.oprcanmerge, 'can_hash', o.oprcanhash,
           'result', o.oprresult::regtype::text, 'function', o.oprcode::regprocedure::text,
           'restriction', o.oprrest::regprocedure::text,
           'join', o.oprjoin::regprocedure::text,
           'owner_is_current_user', o.oprowner = role.oid)::text
    FROM pg_catalog.pg_operator o CROSS JOIN schema_info s CROSS JOIN expected_owner role
   WHERE o.oprnamespace = s.oid
  UNION ALL
  SELECT 'operator_family', am.amname || '.' || f.opfname,
         pg_catalog.jsonb_build_object('owner_is_current_user', f.opfowner = role.oid)::text
    FROM pg_catalog.pg_opfamily f JOIN pg_catalog.pg_am am ON am.oid = f.opfmethod
    CROSS JOIN schema_info s CROSS JOIN expected_owner role WHERE f.opfnamespace = s.oid
  UNION ALL
  SELECT 'operator_class', am.amname || '.' || c.opcname,
         pg_catalog.jsonb_build_object(
           'family', fns.nspname || '.' || f.opfname, 'input', c.opcintype::regtype::text,
           'default', c.opcdefault, 'key_type', c.opckeytype::regtype::text,
           'owner_is_current_user', c.opcowner = role.oid)::text
    FROM pg_catalog.pg_opclass c JOIN pg_catalog.pg_am am ON am.oid = c.opcmethod
    JOIN pg_catalog.pg_opfamily f ON f.oid = c.opcfamily
    JOIN pg_catalog.pg_namespace fns ON fns.oid = f.opfnamespace
    CROSS JOIN schema_info s CROSS JOIN expected_owner role WHERE c.opcnamespace = s.oid
  UNION ALL
  SELECT 'text_search_parser', p.prsname,
         pg_catalog.jsonb_build_object(
           'start', p.prsstart::regprocedure::text, 'token', p.prstoken::regprocedure::text,
           'end', p.prsend::regprocedure::text, 'headline', p.prsheadline::regprocedure::text,
           'lextype', p.prslextype::regprocedure::text)::text
    FROM pg_catalog.pg_ts_parser p CROSS JOIN schema_info s WHERE p.prsnamespace = s.oid
  UNION ALL
  SELECT 'text_search_dictionary', d.dictname,
         pg_catalog.jsonb_build_object(
           'template', tn.nspname || '.' || t.tmplname, 'options', d.dictinitoption,
           'owner_is_current_user', d.dictowner = role.oid)::text
    FROM pg_catalog.pg_ts_dict d JOIN pg_catalog.pg_ts_template t ON t.oid = d.dicttemplate
    JOIN pg_catalog.pg_namespace tn ON tn.oid = t.tmplnamespace
    CROSS JOIN schema_info s CROSS JOIN expected_owner role WHERE d.dictnamespace = s.oid
  UNION ALL
  SELECT 'text_search_template', t.tmplname,
         pg_catalog.jsonb_build_object(
           'init', t.tmplinit::regprocedure::text, 'lexize', t.tmpllexize::regprocedure::text)::text
    FROM pg_catalog.pg_ts_template t CROSS JOIN schema_info s WHERE t.tmplnamespace = s.oid
  UNION ALL
  SELECT 'text_search_configuration', c.cfgname,
         pg_catalog.jsonb_build_object(
           'parser', pn.nspname || '.' || p.prsname,
           'owner_is_current_user', c.cfgowner = role.oid)::text
    FROM pg_catalog.pg_ts_config c JOIN pg_catalog.pg_ts_parser p ON p.oid = c.cfgparser
    JOIN pg_catalog.pg_namespace pn ON pn.oid = p.prsnamespace
    CROSS JOIN schema_info s CROSS JOIN expected_owner role WHERE c.cfgnamespace = s.oid
  UNION ALL
  SELECT 'text_search_mapping', c.cfgname || '.' || m.maptokentype::text || '.' || m.mapseqno::text,
         pg_catalog.jsonb_build_object('dictionary', dn.nspname || '.' || d.dictname)::text
    FROM pg_catalog.pg_ts_config_map m JOIN pg_catalog.pg_ts_config c ON c.oid = m.mapcfg
    JOIN pg_catalog.pg_ts_dict d ON d.oid = m.mapdict
    JOIN pg_catalog.pg_namespace dn ON dn.oid = d.dictnamespace
    CROSS JOIN schema_info s WHERE c.cfgnamespace = s.oid
  UNION ALL
  SELECT 'statistics', e.stxname,
         pg_catalog.jsonb_build_object(
           'relation', c.relname, 'keys', e.stxkeys::text, 'kind', e.stxkind,
           'expressions', pg_catalog.pg_get_expr(e.stxexprs, e.stxrelid, true),
           'owner_is_current_user', e.stxowner = role.oid)::text
    FROM pg_catalog.pg_statistic_ext e JOIN pg_catalog.pg_class c ON c.oid = e.stxrelid
    CROSS JOIN schema_info s CROSS JOIN expected_owner role WHERE e.stxnamespace = s.oid
  UNION ALL
  SELECT 'extension', e.extname,
         pg_catalog.jsonb_build_object(
           'relocatable', e.extrelocatable, 'version', e.extversion,
           'config', e.extconfig, 'condition', e.extcondition,
           'owner_is_current_user', e.extowner = role.oid)::text
    FROM pg_catalog.pg_extension e CROSS JOIN schema_info s CROSS JOIN expected_owner role
   WHERE e.extnamespace = s.oid
  UNION ALL
  SELECT 'cast', source_ns.nspname || '.' || source.typname || '->' ||
                 target_ns.nspname || '.' || target.typname,
         pg_catalog.jsonb_build_object(
           'function', c.castfunc::regprocedure::text, 'context', c.castcontext,
           'method', c.castmethod)::text
    FROM pg_catalog.pg_cast c
    JOIN pg_catalog.pg_type source ON source.oid = c.castsource
    JOIN pg_catalog.pg_namespace source_ns ON source_ns.oid = source.typnamespace
    JOIN pg_catalog.pg_type target ON target.oid = c.casttarget
    JOIN pg_catalog.pg_namespace target_ns ON target_ns.oid = target.typnamespace
    CROSS JOIN schema_info s
   WHERE source.typnamespace = s.oid OR target.typnamespace = s.oid
  UNION ALL
  SELECT 'transform', n.nspname || '.' || t.typname || ':' || language.lanname,
         pg_catalog.jsonb_build_object(
           'from_sql', x.trffromsql::regprocedure::text,
           'to_sql', x.trftosql::regprocedure::text)::text
    FROM pg_catalog.pg_transform x JOIN pg_catalog.pg_type t ON t.oid = x.trftype
    JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
    JOIN pg_catalog.pg_language language ON language.oid = x.trflang
    CROSS JOIN schema_info s WHERE t.typnamespace = s.oid
  UNION ALL
  SELECT 'foreign_table', c.relname,
         pg_catalog.jsonb_build_object(
           'server', server.srvname, 'options', f.ftoptions)::text
    FROM pg_catalog.pg_foreign_table f JOIN pg_catalog.pg_class c ON c.oid = f.ftrelid
    JOIN pg_catalog.pg_foreign_server server ON server.oid = f.ftserver
    CROSS JOIN schema_info s WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'publication_relation', publication.pubname || ':' || c.relname,
         '{}'::jsonb::text
    FROM pg_catalog.pg_publication_rel publication_relation
    JOIN pg_catalog.pg_publication publication ON publication.oid = publication_relation.prpubid
    JOIN pg_catalog.pg_class c ON c.oid = publication_relation.prrelid
    CROSS JOIN schema_info s WHERE c.relnamespace = s.oid
  UNION ALL
  SELECT 'comment', identity.type || ':' || COALESCE(identity.schema, '') || ':' ||
                    COALESCE(identity.name, '') || ':' || identity.identity,
         description.description
    FROM pg_catalog.pg_description description
    CROSS JOIN LATERAL pg_catalog.pg_identify_object(
      description.classoid, description.objoid, description.objsubid) identity
   WHERE identity.schema = 'rustodon'
      OR (identity.type = 'schema' AND identity.name = 'rustodon')
  UNION ALL
  SELECT 'security_label', label.provider || ':' || identity.type || ':' ||
                           COALESCE(identity.schema, '') || ':' ||
                           COALESCE(identity.name, '') || ':' || identity.identity,
         label.label
    FROM pg_catalog.pg_seclabel label
    CROSS JOIN LATERAL pg_catalog.pg_identify_object(
      label.classoid, label.objoid, label.objsubid) identity
   WHERE identity.schema = 'rustodon'
      OR (identity.type = 'schema' AND identity.name = 'rustodon')
  UNION ALL
  SELECT 'extension_membership', extension.extname || ':schema',
         '{}'::jsonb::text
    FROM pg_catalog.pg_depend dependency
    JOIN pg_catalog.pg_extension extension ON extension.oid = dependency.refobjid
    CROSS JOIN schema_info s
   WHERE dependency.classid = 'pg_catalog.pg_namespace'::regclass
     AND dependency.objid = s.oid AND dependency.deptype = 'e'
  UNION ALL
  SELECT 'extension_membership', extension.extname || ':' || identity.type || ':' ||
         COALESCE(identity.schema, '') || ':' || COALESCE(identity.name, '') || ':' ||
         identity.identity,
         '{}'::jsonb::text
    FROM pg_catalog.pg_depend dependency
    JOIN pg_catalog.pg_extension extension ON extension.oid = dependency.refobjid
    CROSS JOIN LATERAL pg_catalog.pg_identify_object(
      dependency.classid, dependency.objid, dependency.objsubid) identity
    CROSS JOIN schema_info s
   WHERE dependency.deptype = 'e'
     AND ((dependency.classid = 'pg_catalog.pg_class'::regclass
           AND dependency.objid IN (
             SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relnamespace = s.oid))
       OR (dependency.classid = 'pg_catalog.pg_proc'::regclass
           AND dependency.objid IN (
             SELECT p.oid FROM pg_catalog.pg_proc p WHERE p.pronamespace = s.oid))
       OR (dependency.classid = 'pg_catalog.pg_type'::regclass
           AND dependency.objid IN (
             SELECT t.oid FROM pg_catalog.pg_type t WHERE t.typnamespace = s.oid)))
  UNION ALL
  SELECT 'external_dependency',
         dependent.type || ':' || COALESCE(dependent.schema, '') || ':' ||
         COALESCE(dependent.name, '') || ':' || dependent.identity,
         pg_catalog.jsonb_build_object(
           'referenced_type', referenced.type,
           'referenced_schema', referenced.schema,
           'referenced_name', referenced.name,
           'referenced_identity', referenced.identity,
           'type', dependency.deptype)::text
    FROM pg_catalog.pg_depend dependency CROSS JOIN schema_info s
    CROSS JOIN LATERAL pg_catalog.pg_identify_object(
      dependency.classid, dependency.objid, dependency.objsubid) dependent
    CROSS JOIN LATERAL pg_catalog.pg_identify_object(
      dependency.refclassid, dependency.refobjid, dependency.refobjsubid) referenced
   WHERE dependency.refclassid IN (
           'pg_catalog.pg_namespace'::regclass,
           'pg_catalog.pg_class'::regclass,
           'pg_catalog.pg_proc'::regclass,
           'pg_catalog.pg_type'::regclass)
     AND ((dependency.refclassid = 'pg_catalog.pg_namespace'::regclass
           AND dependency.refobjid = s.oid)
       OR (dependency.refclassid = 'pg_catalog.pg_class'::regclass
           AND dependency.refobjid IN (
             SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relnamespace = s.oid))
       OR (dependency.refclassid = 'pg_catalog.pg_proc'::regclass
           AND dependency.refobjid IN (
             SELECT p.oid FROM pg_catalog.pg_proc p WHERE p.pronamespace = s.oid))
       OR (dependency.refclassid = 'pg_catalog.pg_type'::regclass
           AND dependency.refobjid IN (
             SELECT t.oid FROM pg_catalog.pg_type t WHERE t.typnamespace = s.oid)))
     AND NOT ((dependency.classid = 'pg_catalog.pg_class'::regclass
               AND dependency.objid IN (
                 SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relnamespace = s.oid))
            OR (dependency.classid = 'pg_catalog.pg_proc'::regclass
                AND dependency.objid IN (
                  SELECT p.oid FROM pg_catalog.pg_proc p WHERE p.pronamespace = s.oid))
             OR (dependency.classid = 'pg_catalog.pg_type'::regclass
                 AND dependency.objid IN (
                   SELECT t.oid FROM pg_catalog.pg_type t WHERE t.typnamespace = s.oid)))
     AND COALESCE(
           dependent.schema,
           (SELECT namespace.nspname
              FROM pg_catalog.pg_rewrite rule
              JOIN pg_catalog.pg_class relation ON relation.oid = rule.ev_class
              JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace
             WHERE dependency.classid = 'pg_catalog.pg_rewrite'::regclass
               AND rule.oid = dependency.objid),
           (SELECT namespace.nspname
              FROM pg_catalog.pg_constraint constraint_record
              JOIN pg_catalog.pg_namespace namespace
                ON namespace.oid = constraint_record.connamespace
             WHERE dependency.classid = 'pg_catalog.pg_constraint'::regclass
               AND constraint_record.oid = dependency.objid),
           (SELECT namespace.nspname
              FROM pg_catalog.pg_attrdef default_record
              JOIN pg_catalog.pg_class relation ON relation.oid = default_record.adrelid
              JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace
             WHERE dependency.classid = 'pg_catalog.pg_attrdef'::regclass
               AND default_record.oid = dependency.objid),
           (SELECT namespace.nspname
              FROM pg_catalog.pg_trigger trigger
              JOIN pg_catalog.pg_class relation ON relation.oid = trigger.tgrelid
              JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace
             WHERE dependency.classid = 'pg_catalog.pg_trigger'::regclass
               AND trigger.oid = dependency.objid)) NOT IN ('rustodon', 'pg_toast')
)
SELECT kind || E'\t' || name || E'\t' || definition
  FROM entries ORDER BY kind COLLATE "C", name COLLATE "C", definition COLLATE "C"
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRecord {
    pub version: i64,
    pub checksum: [u8; 32],
}

impl MigrationRecord {
    #[must_use]
    pub fn known(version: i64) -> Option<Self> {
        migration(version).map(|migration| Self {
            version,
            checksum: migration.checksum(),
        })
    }
}

#[derive(Debug)]
pub enum MigrationError {
    Sqlx(sqlx::Error),
    UnknownVersion(i64),
    ChecksumMismatch(i64),
    InvalidHistory,
    UnversionedSchema,
    SchemaDrift(String),
    UnsupportedMastodonSchema(String),
    MigrationRequired,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlx(error) => {
                write!(
                    formatter,
                    "PostgreSQL rejected the operational migration: {error}"
                )
            }
            Self::UnknownVersion(version) => {
                write!(formatter, "unsupported Rustodon schema version {version}")
            }
            Self::ChecksumMismatch(version) => {
                write!(
                    formatter,
                    "Rustodon schema migration {version} has an unknown checksum"
                )
            }
            Self::InvalidHistory => formatter.write_str("Rustodon schema history is not a prefix"),
            Self::UnversionedSchema => {
                formatter.write_str("the rustodon schema contains unversioned objects")
            }
            Self::SchemaDrift(detail) => write!(formatter, "Rustodon schema drift: {detail}"),
            Self::UnsupportedMastodonSchema(detail) => {
                write!(formatter, "unsupported Mastodon schema: {detail}")
            }
            Self::MigrationRequired => formatter
                .write_str("the Rustodon operational schema requires an explicit migration"),
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlx(error) => Some(error),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for MigrationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

struct Migration {
    version: i64,
    sql: &'static str,
}

impl Migration {
    fn checksum(&self) -> [u8; 32] {
        Sha256::digest(self.sql.as_bytes()).into()
    }
}

fn migration(version: i64) -> Option<Migration> {
    match version {
        1 => Some(Migration {
            version,
            sql: MIGRATION_1_SQL,
        }),
        2 => Some(Migration {
            version,
            sql: MIGRATION_2_SQL,
        }),
        3 => Some(Migration {
            version,
            sql: MIGRATION_3_SQL,
        }),
        4 => Some(Migration {
            version,
            sql: MIGRATION_4_SQL,
        }),
        _ => None,
    }
}

/// Returns the known migrations not present in an exact applied prefix.
///
/// # Errors
///
/// Rejects unknown, reordered, missing, or checksum-mismatched migration records.
pub fn migration_plan(applied: &[MigrationRecord]) -> Result<Vec<i64>, MigrationError> {
    for (index, record) in applied.iter().enumerate() {
        let Ok(expected_version) = i64::try_from(index + 1) else {
            return Err(MigrationError::InvalidHistory);
        };
        if record.version > CURRENT_VERSION {
            return Err(MigrationError::UnknownVersion(record.version));
        }
        if record.version != expected_version {
            return Err(MigrationError::InvalidHistory);
        }
        let expected = MigrationRecord::known(record.version)
            .ok_or(MigrationError::UnknownVersion(record.version))?;
        if record.checksum != expected.checksum {
            return Err(MigrationError::ChecksumMismatch(record.version));
        }
    }
    ((applied.len() + 1)..=usize::try_from(CURRENT_VERSION).unwrap_or_default())
        .map(i64::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| MigrationError::InvalidHistory)
}

/// Creates or upgrades the Rustodon-owned operational schema transactionally.
///
/// # Errors
///
/// Rejects unknown or drifted schemas and propagates `PostgreSQL` migration failures.
pub async fn migrate(connection: &mut PgConnection) -> Result<(), MigrationError> {
    let mut transaction = connection.begin().await?;
    let result = migrate_transaction(&mut transaction).await;
    match result {
        Ok(()) => transaction.commit().await.map_err(MigrationError::from),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

/// Validates the exact current operational schema without applying DDL.
///
/// # Errors
///
/// Rejects missing, outdated, unknown, or drifted operational schemas.
pub async fn validate(connection: &mut PgConnection) -> Result<(), MigrationError> {
    let mut transaction = connection.begin().await?;
    let result = async {
        sqlx::raw_sql(
            "SET TRANSACTION READ ONLY; SET LOCAL lock_timeout TO '10s'; \
             SET LOCAL statement_timeout TO '60s'",
        )
        .execute(&mut *transaction)
        .await?;
        crate::preflight::validate_supported_mastodon_schema_in_transaction(&mut transaction)
            .await
            .map_err(MigrationError::UnsupportedMastodonSchema)?;
        let schema_exists = sqlx::query_scalar::<_, bool>(
            "SELECT pg_catalog.to_regnamespace('rustodon') IS NOT NULL",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if !schema_exists {
            return Err(MigrationError::MigrationRequired);
        }
        validate_ledger(&mut transaction).await?;
        let applied = load_records(&mut transaction).await?;
        if !migration_plan(&applied)?.is_empty() {
            return Err(MigrationError::MigrationRequired);
        }
        validate_runtime_role(&mut transaction, true).await?;
        validate_catalog(&mut transaction).await
    }
    .await;
    let rollback = transaction.rollback().await;
    match (result, rollback) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(MigrationError::Sqlx(error)),
    }
}

async fn migrate_transaction(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), MigrationError> {
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock( \
           pg_catalog.hashtext(pg_catalog.current_database()), \
           pg_catalog.hashtext('rustodon:migrations'))",
    )
    .execute(&mut **transaction)
    .await?;
    let database_name = sqlx::query_scalar::<_, String>("SELECT pg_catalog.current_database()")
        .fetch_one(&mut **transaction)
        .await?;
    sqlx::query("SELECT pg_catalog.pg_advisory_xact_lock($1)")
        .bind(2_053_462_845_i64 * i64::from(crc32(database_name.as_bytes())))
        .execute(&mut **transaction)
        .await?;
    sqlx::raw_sql("SET LOCAL lock_timeout TO '10s'; SET LOCAL statement_timeout TO '60s'")
        .execute(&mut **transaction)
        .await?;
    crate::preflight::validate_supported_mastodon_schema_in_transaction(transaction)
        .await
        .map_err(MigrationError::UnsupportedMastodonSchema)?;

    bootstrap(transaction).await?;
    validate_ledger(transaction).await?;
    let applied = load_records(transaction).await?;
    let plan = migration_plan(&applied)?;
    let runtime_role = if plan.is_empty() {
        None
    } else {
        discover_runtime_role(transaction).await?
    };
    for version in plan {
        let migration = migration(version).ok_or(MigrationError::UnknownVersion(version))?;
        sqlx::raw_sql(migration.sql)
            .execute(&mut **transaction)
            .await?;
        sqlx::query(
            "INSERT INTO rustodon.schema_migrations (version, checksum, owner) \
             VALUES ($1, $2, CURRENT_USER)",
        )
        .bind(migration.version)
        .bind(migration.checksum().as_slice())
        .execute(&mut **transaction)
        .await?;
        grant_runtime_privileges_for_migration(transaction, version, runtime_role.as_deref())
            .await?;
    }
    validate_runtime_role(transaction, false).await?;
    validate_catalog(transaction).await?;
    crate::preflight::validate_supported_mastodon_schema_in_transaction(transaction)
        .await
        .map_err(MigrationError::UnsupportedMastodonSchema)
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |mut crc, byte| {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
        crc
    })
}

async fn bootstrap(transaction: &mut Transaction<'_, Postgres>) -> Result<(), MigrationError> {
    let schema_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname = 'rustodon')",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if !schema_exists {
        sqlx::raw_sql(BOOTSTRAP_SQL)
            .execute(&mut **transaction)
            .await?;
        return Ok(());
    }
    let ledger_exists = sqlx::query_scalar::<_, bool>(
        "SELECT pg_catalog.to_regclass('rustodon.schema_migrations') IS NOT NULL",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if ledger_exists {
        return Ok(());
    }
    let entries = catalog_entries(transaction).await?;
    if entries.len() != 1 || !entries[0].starts_with("schema\trustodon\t") {
        return Err(MigrationError::UnversionedSchema);
    }
    sqlx::raw_sql(
        "CREATE TABLE rustodon.schema_migrations ( \
          version bigint NOT NULL, checksum bytea NOT NULL, owner name NOT NULL, \
          applied_at timestamp with time zone NOT NULL DEFAULT clock_timestamp(), \
          CONSTRAINT schema_migrations_pkey PRIMARY KEY (version), \
          CONSTRAINT schema_migrations_version_check CHECK (version > 0), \
          CONSTRAINT schema_migrations_checksum_check CHECK (octet_length(checksum) = 32)); \
         REVOKE ALL ON SCHEMA rustodon FROM PUBLIC; \
         REVOKE ALL ON TABLE rustodon.schema_migrations FROM PUBLIC",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn validate_ledger(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), MigrationError> {
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT a.attname::text || ':' || pg_catalog.format_type(a.atttypid, a.atttypmod) || ':' || a.attnotnull::text \
         FROM pg_catalog.pg_attribute a \
         WHERE a.attrelid = 'rustodon.schema_migrations'::regclass AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let expected = [
        "version:bigint:true",
        "checksum:bytea:true",
        "owner:name:true",
        "applied_at:timestamp with time zone:true",
    ];
    if columns != expected {
        return Err(MigrationError::SchemaDrift(
            "schema_migrations columns changed".to_owned(),
        ));
    }
    let owner_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(DISTINCT owner) FROM rustodon.schema_migrations",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if owner_count > 1 {
        return Err(MigrationError::SchemaDrift(
            "schema_migrations owner changed".to_owned(),
        ));
    }
    Ok(())
}

async fn load_records(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Vec<MigrationRecord>, MigrationError> {
    let rows =
        sqlx::query("SELECT version, checksum FROM rustodon.schema_migrations ORDER BY version")
            .fetch_all(&mut **transaction)
            .await?;
    rows.into_iter()
        .map(|row| {
            let version = row.try_get("version")?;
            let checksum = row.try_get::<Vec<u8>, _>("checksum")?;
            let checksum = checksum.try_into().map_err(|_| {
                MigrationError::SchemaDrift("migration checksum width changed".to_owned())
            })?;
            Ok(MigrationRecord { version, checksum })
        })
        .collect()
}

async fn validate_catalog(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), MigrationError> {
    let tables = object_names(transaction, "r").await?;
    if tables != TABLES {
        return Err(MigrationError::SchemaDrift(
            "operational table inventory changed".to_owned(),
        ));
    }
    let sequences = object_names(transaction, "S").await?;
    if sequences != SEQUENCES {
        return Err(MigrationError::SchemaDrift(
            "operational sequence inventory changed".to_owned(),
        ));
    }
    let indexes = object_names(transaction, "i").await?;
    if indexes != INDEXES {
        return Err(MigrationError::SchemaDrift(
            "operational index inventory changed".to_owned(),
        ));
    }
    let unexpected_columns = sqlx::query_scalar::<_, String>(
        "WITH expected(table_name, column_count) AS (VALUES \
           ('domain_health', 7), ('durable_jobs', 15), ('heartbeats', 6), \
           ('idempotency_keys', 6), ('ordering_markers', 6), \
           ('outbox_events', 6), ('rate_limit_windows', 4), \
           ('remote_fetch_leases', 3), \
           ('schema_migrations', 4)) \
         SELECT expected.table_name FROM expected \
         LEFT JOIN ( \
           SELECT c.relname::text AS table_name, count(*)::integer AS column_count \
           FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
           JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid \
           WHERE n.nspname = 'rustodon' AND c.relkind = 'r' \
             AND a.attnum > 0 AND NOT a.attisdropped GROUP BY c.relname) actual \
           USING (table_name) \
         WHERE actual.column_count IS DISTINCT FROM expected.column_count \
         ORDER BY expected.table_name",
    )
    .fetch_all(&mut **transaction)
    .await?;
    if !unexpected_columns.is_empty() {
        return Err(MigrationError::SchemaDrift(format!(
            "column inventory changed for {}",
            unexpected_columns.join(", ")
        )));
    }
    let entries = catalog_entries(transaction).await?;
    let checksum: [u8; 32] = Sha256::digest(entries.join("\n").as_bytes()).into();
    if checksum != EXPECTED_CATALOG_SHA256 {
        let runtime_role = sqlx::query_scalar::<_, Option<String>>(
            "SELECT pg_catalog.current_setting('rustodon.runtime_role', true)",
        )
        .fetch_one(&mut **transaction)
        .await?;
        let writer_role = sqlx::query_scalar::<_, Option<String>>(
            "SELECT pg_catalog.current_setting('rustodon.writer_role', true)",
        )
        .fetch_one(&mut **transaction)
        .await?;
        let expected_owner = sqlx::query_scalar::<_, Option<String>>(
            "SELECT pg_catalog.current_setting('rustodon.expected_owner', true)",
        )
        .fetch_one(&mut **transaction)
        .await?;
        return Err(MigrationError::SchemaDrift(format!(
            "catalog fingerprint {} is not recognized (runtime_role={runtime_role:?}, writer_role={writer_role:?}, expected_owner={expected_owner:?}, entries={})",
            hex(&checksum),
            entries.len()
        )));
    }
    Ok(())
}

async fn validate_runtime_role(
    transaction: &mut Transaction<'_, Postgres>,
    required_for_current_user: bool,
) -> Result<(), MigrationError> {
    let Some(runtime_role) = discover_runtime_role(transaction).await? else {
        if required_for_current_user {
            return Err(MigrationError::SchemaDrift(
                "runtime role is missing".to_owned(),
            ));
        }
        sqlx::query("SELECT pg_catalog.set_config('rustodon.runtime_role', '', true)")
            .execute(&mut **transaction)
            .await?;
        return Ok(());
    };
    validate_runtime_role_attributes(transaction, &runtime_role).await?;
    validate_mastodon_read_privileges(transaction, &runtime_role).await?;
    let privileges = runtime_role_privileges(transaction, &runtime_role).await?;
    let expected = expected_runtime_role_privileges();
    if privileges != expected {
        return Err(MigrationError::SchemaDrift(
            "runtime role privileges changed".to_owned(),
        ));
    }
    if required_for_current_user
        && (runtime_role != current_user(transaction).await?
            || current_user(transaction).await? != session_user(transaction).await?)
    {
        return Err(MigrationError::SchemaDrift(
            "runtime process is not directly connected as the operational role".to_owned(),
        ));
    }
    sqlx::query("SELECT pg_catalog.set_config('rustodon.runtime_role', $1, true)")
        .bind(runtime_role)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn discover_runtime_role(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Option<String>, MigrationError> {
    let candidates = sqlx::query_scalar::<_, String>(
        "SELECT role.rolname::text FROM pg_catalog.pg_class relation \
         JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
         CROSS JOIN LATERAL pg_catalog.aclexplode(relation.relacl) acl \
         JOIN pg_catalog.pg_roles role ON role.oid = acl.grantee \
         WHERE namespace.nspname = 'rustodon' AND relation.relname = 'schema_migrations' \
           AND acl.privilege_type = 'SELECT' AND role.rolname <> ( \
             SELECT owner::text FROM rustodon.schema_migrations ORDER BY version LIMIT 1) \
         ORDER BY role.rolname COLLATE \"C\"",
    )
    .fetch_all(&mut **transaction)
    .await?;
    match candidates.as_slice() {
        [] => Ok(None),
        [runtime_role] => Ok(Some(runtime_role.clone())),
        _ => Err(MigrationError::SchemaDrift(
            "multiple runtime roles are granted operational access".to_owned(),
        )),
    }
}

async fn grant_runtime_privileges_for_migration(
    transaction: &mut Transaction<'_, Postgres>,
    version: i64,
    runtime_role: Option<&str>,
) -> Result<(), MigrationError> {
    let Some(runtime_role) = runtime_role else {
        return Ok(());
    };
    let grants: &[(&str, &str)] = match version {
        1 => &[
            (
                "SELECT, INSERT, UPDATE, DELETE",
                "TABLE rustodon.durable_jobs, rustodon.outbox_events, rustodon.idempotency_keys, rustodon.ordering_markers, rustodon.domain_health, rustodon.heartbeats",
            ),
            (
                "USAGE",
                "SEQUENCE rustodon.durable_jobs_id_seq, rustodon.outbox_events_id_seq",
            ),
        ],
        2 => &[(
            "SELECT, INSERT, UPDATE, DELETE",
            "TABLE rustodon.rate_limit_windows",
        )],
        3 => &[(
            "SELECT, INSERT, DELETE",
            "TABLE rustodon.remote_fetch_leases",
        )],
        _ => &[],
    };
    let quoted_role = sqlx::query_scalar::<_, String>("SELECT pg_catalog.quote_ident($1)")
        .bind(runtime_role)
        .fetch_one(&mut **transaction)
        .await?;
    for (privileges, objects) in grants {
        sqlx::query(&format!("GRANT {privileges} ON {objects} TO {quoted_role}"))
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn validate_mastodon_read_privileges(
    transaction: &mut Transaction<'_, Postgres>,
    runtime_role: &str,
) -> Result<(), MigrationError> {
    let relations = crate::preflight::V1_CRITICAL_TABLES
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let missing = sqlx::query_scalar::<_, String>(
        "SELECT relation_name FROM unnest($1::text[]) relation_name \
         WHERE NOT pg_catalog.has_table_privilege( \
           $2, pg_catalog.format('%I.%I', 'public', relation_name), 'SELECT') \
         ORDER BY relation_name COLLATE \"C\"",
    )
    .bind(&relations)
    .bind(runtime_role)
    .fetch_all(&mut **transaction)
    .await?;
    if !missing.is_empty() {
        return Err(MigrationError::SchemaDrift(
            "runtime role is missing required Mastodon reads".to_owned(),
        ));
    }
    Ok(())
}

async fn validate_runtime_role_attributes(
    transaction: &mut Transaction<'_, Postgres>,
    runtime_role: &str,
) -> Result<(), MigrationError> {
    let role_is_restricted = sqlx::query_scalar::<_, bool>(
        "SELECT role.rolcanlogin AND NOT role.rolsuper AND NOT role.rolinherit \
                AND NOT role.rolcreaterole AND NOT role.rolcreatedb AND NOT role.rolreplication \
                AND NOT role.rolbypassrls \
                AND role.rolname <> ledger.owner::text \
                AND NOT pg_catalog.pg_has_role(role.oid, \
                  (SELECT datdba FROM pg_catalog.pg_database \
                   WHERE datname = pg_catalog.current_database()), 'MEMBER') \
                AND NOT pg_catalog.has_database_privilege( \
                  role.oid, pg_catalog.current_database(), 'CREATE') \
                AND NOT pg_catalog.has_database_privilege( \
                  role.oid, pg_catalog.current_database(), 'TEMP') \
                AND NOT pg_catalog.has_schema_privilege(role.oid, 'public', 'CREATE') \
                AND NOT EXISTS ( \
                  SELECT 1 FROM pg_catalog.pg_class relation \
                  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
                  WHERE namespace.nspname = 'public' AND relation.relkind IN ('r', 'p', 'v', 'm', 'f') \
                    AND (relation.relowner = role.oid OR EXISTS ( \
                      SELECT 1 FROM pg_catalog.aclexplode(relation.relacl) acl \
                      WHERE acl.grantee IN (0, role.oid) AND acl.privilege_type IN ( \
                        'INSERT', 'UPDATE', 'DELETE', 'TRUNCATE', 'REFERENCES', 'TRIGGER')) \
                      OR EXISTS ( \
                        SELECT 1 FROM pg_catalog.pg_attribute attribute \
                        CROSS JOIN LATERAL pg_catalog.aclexplode(attribute.attacl) acl \
                        WHERE attribute.attrelid = relation.oid \
                          AND acl.grantee IN (0, role.oid) \
                          AND acl.privilege_type IN ('INSERT', 'UPDATE', 'REFERENCES')))) \
                AND NOT EXISTS ( \
                  SELECT 1 FROM pg_catalog.pg_class sequence_record \
                  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = sequence_record.relnamespace \
                  WHERE namespace.nspname = 'public' AND sequence_record.relkind = 'S' \
                    AND (sequence_record.relowner = role.oid OR EXISTS ( \
                      SELECT 1 FROM pg_catalog.aclexplode(sequence_record.relacl) acl \
                      WHERE acl.grantee IN (0, role.oid) \
                        AND acl.privilege_type IN ('USAGE', 'UPDATE')))) \
                AND NOT EXISTS ( \
                  SELECT 1 FROM pg_catalog.pg_proc function_record \
                  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = function_record.pronamespace \
                  WHERE namespace.nspname = 'public' AND function_record.proowner = role.oid) \
                AND NOT EXISTS ( \
                  SELECT 1 FROM pg_catalog.pg_proc function_record \
                  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = function_record.pronamespace \
                  WHERE namespace.nspname = 'public' AND function_record.prosecdef \
                    AND pg_catalog.has_function_privilege(role.oid, function_record.oid, 'EXECUTE')) \
                AND NOT EXISTS ( \
                  SELECT 1 FROM pg_catalog.pg_type type_record \
                  JOIN pg_catalog.pg_namespace namespace ON namespace.oid = type_record.typnamespace \
                  WHERE namespace.nspname = 'public' AND type_record.typowner = role.oid) \
                AND NOT EXISTS ( \
                  SELECT 1 FROM pg_catalog.pg_auth_members membership \
                  WHERE membership.member = role.oid OR membership.roleid = role.oid) \
          FROM pg_catalog.pg_roles role \
          CROSS JOIN ( \
            SELECT owner FROM rustodon.schema_migrations ORDER BY version LIMIT 1) ledger \
         WHERE role.rolname = $1",
    )
    .bind(runtime_role)
    .fetch_one(&mut **transaction)
    .await?;
    if !role_is_restricted {
        return Err(MigrationError::SchemaDrift(
            "runtime role is missing or privileged".to_owned(),
        ));
    }
    Ok(())
}

async fn runtime_role_privileges(
    transaction: &mut Transaction<'_, Postgres>,
    runtime_role: &str,
) -> Result<Vec<String>, MigrationError> {
    Ok(sqlx::query_scalar::<_, String>(
        "WITH runtime AS ( \
           SELECT oid FROM pg_catalog.pg_roles WHERE rolname = $1), \
         privileges AS ( \
           SELECT 'schema:' || acl.privilege_type || ':' || acl.is_grantable::text AS privilege \
           FROM pg_catalog.pg_namespace namespace CROSS JOIN runtime \
           CROSS JOIN LATERAL pg_catalog.aclexplode(namespace.nspacl) acl \
           WHERE namespace.nspname = 'rustodon' AND acl.grantee = runtime.oid \
           UNION ALL \
           SELECT relation.relname || ':' || acl.privilege_type || ':' || \
                  acl.is_grantable::text \
           FROM pg_catalog.pg_class relation \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           CROSS JOIN runtime \
           CROSS JOIN LATERAL pg_catalog.aclexplode(relation.relacl) acl \
           WHERE namespace.nspname = 'rustodon' AND acl.grantee = runtime.oid \
           UNION ALL \
           SELECT relation.relname || '.' || attribute.attname || ':' || acl.privilege_type || \
                  ':' || acl.is_grantable::text \
           FROM pg_catalog.pg_attribute attribute \
           JOIN pg_catalog.pg_class relation ON relation.oid = attribute.attrelid \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           CROSS JOIN runtime \
           CROSS JOIN LATERAL pg_catalog.aclexplode(attribute.attacl) acl \
           WHERE namespace.nspname = 'rustodon' AND acl.grantee = runtime.oid \
           UNION ALL \
           SELECT 'function:' || function_record.proname || ':' || acl.privilege_type || ':' || \
                  acl.is_grantable::text \
           FROM pg_catalog.pg_proc function_record \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = function_record.pronamespace \
           CROSS JOIN runtime \
           CROSS JOIN LATERAL pg_catalog.aclexplode(function_record.proacl) acl \
           WHERE namespace.nspname = 'rustodon' AND acl.grantee = runtime.oid \
           UNION ALL \
           SELECT 'type:' || type_record.typname || ':' || acl.privilege_type || ':' || \
                  acl.is_grantable::text \
           FROM pg_catalog.pg_type type_record \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = type_record.typnamespace \
           CROSS JOIN runtime \
           CROSS JOIN LATERAL pg_catalog.aclexplode(type_record.typacl) acl \
           WHERE namespace.nspname = 'rustodon' AND acl.grantee = runtime.oid \
           UNION ALL \
           SELECT 'default:' || default_acl.defaclobjtype || ':' || acl.privilege_type || ':' || \
                  acl.is_grantable::text \
           FROM pg_catalog.pg_default_acl default_acl \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = default_acl.defaclnamespace \
           CROSS JOIN runtime \
           CROSS JOIN LATERAL pg_catalog.aclexplode(default_acl.defaclacl) acl \
           WHERE namespace.nspname = 'rustodon' AND acl.grantee = runtime.oid) \
         SELECT privilege FROM privileges ORDER BY privilege COLLATE \"C\"",
    )
    .bind(runtime_role)
    .fetch_all(&mut **transaction)
    .await?)
}

fn expected_runtime_role_privileges() -> Vec<String> {
    [
        "domain_health:DELETE:false",
        "domain_health:INSERT:false",
        "domain_health:SELECT:false",
        "domain_health:UPDATE:false",
        "durable_jobs:DELETE:false",
        "durable_jobs:INSERT:false",
        "durable_jobs:SELECT:false",
        "durable_jobs:UPDATE:false",
        "durable_jobs_id_seq:USAGE:false",
        "heartbeats:DELETE:false",
        "heartbeats:INSERT:false",
        "heartbeats:SELECT:false",
        "heartbeats:UPDATE:false",
        "idempotency_keys:DELETE:false",
        "idempotency_keys:INSERT:false",
        "idempotency_keys:SELECT:false",
        "idempotency_keys:UPDATE:false",
        "ordering_markers:DELETE:false",
        "ordering_markers:INSERT:false",
        "ordering_markers:SELECT:false",
        "ordering_markers:UPDATE:false",
        "outbox_events:DELETE:false",
        "outbox_events:INSERT:false",
        "outbox_events:SELECT:false",
        "outbox_events:UPDATE:false",
        "outbox_events_id_seq:USAGE:false",
        "rate_limit_windows:DELETE:false",
        "rate_limit_windows:INSERT:false",
        "rate_limit_windows:SELECT:false",
        "rate_limit_windows:UPDATE:false",
        "remote_fetch_leases:DELETE:false",
        "remote_fetch_leases:INSERT:false",
        "remote_fetch_leases:SELECT:false",
        "schema:USAGE:false",
        "schema_migrations:SELECT:false",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

async fn current_user(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<String, MigrationError> {
    Ok(sqlx::query_scalar("SELECT CURRENT_USER::text")
        .fetch_one(&mut **transaction)
        .await?)
}

async fn session_user(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<String, MigrationError> {
    Ok(sqlx::query_scalar("SELECT SESSION_USER::text")
        .fetch_one(&mut **transaction)
        .await?)
}

async fn catalog_entries(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Vec<String>, MigrationError> {
    let owner = if sqlx::query_scalar::<_, bool>(
        "SELECT pg_catalog.to_regclass('rustodon.schema_migrations') IS NOT NULL",
    )
    .fetch_one(&mut **transaction)
    .await?
    {
        sqlx::query_scalar::<_, String>(
            "SELECT owner::text FROM rustodon.schema_migrations ORDER BY version LIMIT 1",
        )
        .fetch_optional(&mut **transaction)
        .await?
        .unwrap_or_else(|| "__rustodon_missing_owner__".to_owned())
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT pg_catalog.pg_get_userbyid(nspowner) \
             FROM pg_catalog.pg_namespace WHERE nspname = 'rustodon'",
        )
        .fetch_one(&mut **transaction)
        .await?
    };
    sqlx::query("SELECT pg_catalog.set_config('rustodon.expected_owner', $1, true)")
        .bind(owner)
        .execute(&mut **transaction)
        .await?;
    Ok(sqlx::query_scalar::<_, String>(CATALOG_QUERY)
        .fetch_all(&mut **transaction)
        .await?)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        },
    )
}

async fn object_names(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &str,
) -> Result<Vec<String>, MigrationError> {
    Ok(sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'rustodon' AND c.relkind = $1::\"char\" ORDER BY c.relname",
    )
    .bind(kind)
    .fetch_all(&mut **transaction)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn crc32_matches_active_record_migrator_hash() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
