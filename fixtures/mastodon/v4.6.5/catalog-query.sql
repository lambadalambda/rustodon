\set ON_ERROR_STOP on

WITH catalog_entries AS (
  SELECT
    'relation' AS object_kind,
    c.relname AS object_name,
    json_build_object(
      'schema', n.nspname,
      'name', c.relname,
      'kind', c.relkind,
      'persistence', c.relpersistence,
      'partitioned', c.relispartition,
      'row_security', c.relrowsecurity
    )::text AS definition
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')

  UNION ALL

  SELECT
    'column',
    c.relname || '.' || a.attname,
    json_build_object(
      'schema', n.nspname,
      'relation', c.relname,
      'position', a.attnum,
      'name', a.attname,
      'type', pg_catalog.format_type(a.atttypid, a.atttypmod),
      'not_null', a.attnotnull,
      'default', pg_get_expr(d.adbin, d.adrelid, true),
      'identity', a.attidentity,
      'generated', a.attgenerated,
      'collation', CASE WHEN a.attcollation = 0 THEN NULL ELSE coll.collname END
    )::text
  FROM pg_attribute a
  JOIN pg_class c ON c.oid = a.attrelid
  JOIN pg_namespace n ON n.oid = c.relnamespace
  LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
  LEFT JOIN pg_collation coll ON coll.oid = a.attcollation
  WHERE n.nspname = 'public'
    AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
    AND a.attnum > 0
    AND NOT a.attisdropped

  UNION ALL

  SELECT
    'constraint',
    COALESCE(c.relname || '.', '') || con.conname,
    json_build_object(
      'schema', n.nspname,
      'relation', c.relname,
      'name', con.conname,
      'type', con.contype,
      'deferrable', con.condeferrable,
      'initially_deferred', con.condeferred,
      'validated', con.convalidated,
      'definition', pg_get_constraintdef(con.oid, true)
    )::text
  FROM pg_constraint con
  JOIN pg_namespace n ON n.oid = con.connamespace
  LEFT JOIN pg_class c ON c.oid = con.conrelid
  WHERE n.nspname = 'public'

  UNION ALL

  SELECT
    'index',
    table_name || '.' || index_name,
    json_build_object(
      'schema', schemaname,
      'relation', table_name,
      'name', index_name,
      'unique', indisunique,
      'primary', indisprimary,
      'valid', indisvalid,
      'ready', indisready,
      'definition', pg_get_indexdef(indexrelid, 0, true)
    )::text
  FROM (
    SELECT
      ns.nspname AS schemaname,
      table_class.relname AS table_name,
      index_class.relname AS index_name,
      idx.indexrelid,
      idx.indisunique,
      idx.indisprimary,
      idx.indisvalid,
      idx.indisready
    FROM pg_index idx
    JOIN pg_class table_class ON table_class.oid = idx.indrelid
    JOIN pg_class index_class ON index_class.oid = idx.indexrelid
    JOIN pg_namespace ns ON ns.oid = table_class.relnamespace
    WHERE ns.nspname = 'public'
  ) indexes

  UNION ALL

  SELECT
    CASE WHEN c.relkind = 'm' THEN 'materialized_view' ELSE 'view' END,
    c.relname,
    json_build_object(
      'schema', n.nspname,
      'name', c.relname,
      'definition', pg_get_viewdef(c.oid, true)
    )::text
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = 'public' AND c.relkind IN ('v', 'm')

  UNION ALL

  SELECT
    'sequence',
    c.relname,
    json_build_object(
      'schema', n.nspname,
      'name', c.relname,
      'data_type', pg_catalog.format_type(s.seqtypid, NULL),
      'start', s.seqstart,
      'increment', s.seqincrement,
      'minimum', s.seqmin,
      'maximum', s.seqmax,
      'cache', s.seqcache,
      'cycle', s.seqcycle
    )::text
  FROM pg_sequence s
  JOIN pg_class c ON c.oid = s.seqrelid
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = 'public'

  UNION ALL

  SELECT
    'extension',
    e.extname,
    json_build_object(
      'name', e.extname,
      'version', e.extversion,
      'schema', n.nspname,
      'relocatable', e.extrelocatable
    )::text
  FROM pg_extension e
  JOIN pg_namespace n ON n.oid = e.extnamespace

  UNION ALL

  SELECT
    'function',
    p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')',
    json_build_object(
      'schema', n.nspname,
      'name', p.proname,
      'identity_arguments', pg_get_function_identity_arguments(p.oid),
      'result', pg_get_function_result(p.oid),
      'language', l.lanname,
      'kind', p.prokind,
      'volatility', p.provolatile,
      'parallel', p.proparallel,
      'security_definer', p.prosecdef,
      'leakproof', p.proleakproof,
      'strict', p.proisstrict,
      'definition', pg_get_functiondef(p.oid)
    )::text
  FROM pg_proc p
  JOIN pg_namespace n ON n.oid = p.pronamespace
  JOIN pg_language l ON l.oid = p.prolang
  WHERE n.nspname = 'public'
)
SELECT object_kind || E'\t' || object_name || E'\t' || definition
FROM catalog_entries
ORDER BY object_kind COLLATE "C", object_name COLLATE "C", definition COLLATE "C";
