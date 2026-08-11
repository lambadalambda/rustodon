--
-- PostgreSQL database dump
--

\restrict rustodonMastodon465FixtureDump

-- Dumped from database version 14.23 (Debian 14.23-1.pgdg13+1)
-- Dumped by pg_dump version 14.23 (Debian 14.23-1.pgdg13+1)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: timestamp_id(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.timestamp_id(table_name text) RETURNS bigint
    LANGUAGE plpgsql
    AS $$
  DECLARE
    time_part bigint;
    sequence_base bigint;
    tail bigint;
  BEGIN
    time_part := (((date_part('epoch', now()) * 1000))::bigint << 16);
    sequence_base := (
      'x' || substr(
        md5(table_name || 'rustodon-mastodon-v4.6.5-fixture-salt' || time_part::text),
        1,
        4
      )
    )::bit(16)::bigint;
    tail := ((sequence_base + nextval(table_name || '_id_seq')) & 65535);
    RETURN time_part | tail;
  END
$$;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: account_aliases; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_aliases (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    acct character varying DEFAULT ''::character varying NOT NULL,
    created_at timestamp without time zone NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying DEFAULT ''::character varying NOT NULL
);


--
-- Name: account_aliases_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_aliases_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_aliases_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_aliases_id_seq OWNED BY public.account_aliases.id;


--
-- Name: account_conversations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_conversations (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    conversation_id bigint NOT NULL,
    last_status_id bigint,
    lock_version integer DEFAULT 0 NOT NULL,
    participant_account_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    status_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    unread boolean DEFAULT false NOT NULL
);


--
-- Name: account_conversations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_conversations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_conversations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_conversations_id_seq OWNED BY public.account_conversations.id;


--
-- Name: account_deletion_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_deletion_requests (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_deletion_requests_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_deletion_requests_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_deletion_requests_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_deletion_requests_id_seq OWNED BY public.account_deletion_requests.id;


--
-- Name: account_domain_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_domain_blocks (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    domain character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_domain_blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_domain_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_domain_blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_domain_blocks_id_seq OWNED BY public.account_domain_blocks.id;


--
-- Name: account_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_migrations (
    id bigint NOT NULL,
    account_id bigint,
    acct character varying DEFAULT ''::character varying NOT NULL,
    created_at timestamp without time zone NOT NULL,
    followers_count bigint DEFAULT 0 NOT NULL,
    target_account_id bigint,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_migrations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_migrations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_migrations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_migrations_id_seq OWNED BY public.account_migrations.id;


--
-- Name: account_moderation_notes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_moderation_notes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    content text NOT NULL,
    created_at timestamp without time zone NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_moderation_notes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_moderation_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_moderation_notes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_moderation_notes_id_seq OWNED BY public.account_moderation_notes.id;


--
-- Name: account_notes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_notes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    comment text NOT NULL,
    created_at timestamp without time zone NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_notes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_notes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_notes_id_seq OWNED BY public.account_notes.id;


--
-- Name: account_pins; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_pins (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_pins_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_pins_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_pins_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_pins_id_seq OWNED BY public.account_pins.id;


--
-- Name: account_relationship_severance_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_relationship_severance_events (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    followers_count integer DEFAULT 0 NOT NULL,
    following_count integer DEFAULT 0 NOT NULL,
    relationship_severance_event_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: account_relationship_severance_events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_relationship_severance_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_relationship_severance_events_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_relationship_severance_events_id_seq OWNED BY public.account_relationship_severance_events.id;


--
-- Name: account_stats; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_stats (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    followers_count bigint DEFAULT 0 NOT NULL,
    following_count bigint DEFAULT 0 NOT NULL,
    last_status_at timestamp without time zone,
    statuses_count bigint DEFAULT 0 NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_stats_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_stats_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_stats_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_stats_id_seq OWNED BY public.account_stats.id;


--
-- Name: account_statuses_cleanup_policies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_statuses_cleanup_policies (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    keep_direct boolean DEFAULT true NOT NULL,
    keep_media boolean DEFAULT false NOT NULL,
    keep_pinned boolean DEFAULT true NOT NULL,
    keep_polls boolean DEFAULT false NOT NULL,
    keep_self_bookmark boolean DEFAULT true NOT NULL,
    keep_self_fav boolean DEFAULT true NOT NULL,
    min_favs integer,
    min_reblogs integer,
    min_status_age integer DEFAULT 1209600 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: account_statuses_cleanup_policies_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_statuses_cleanup_policies_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_statuses_cleanup_policies_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_statuses_cleanup_policies_id_seq OWNED BY public.account_statuses_cleanup_policies.id;


--
-- Name: accounts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.accounts (
    id bigint DEFAULT public.timestamp_id('accounts'::text) NOT NULL,
    actor_type character varying,
    also_known_as character varying[],
    attribution_domains character varying[] DEFAULT '{}'::character varying[],
    avatar_content_type character varying,
    avatar_description character varying DEFAULT ''::character varying NOT NULL,
    avatar_file_name character varying,
    avatar_file_size integer,
    avatar_remote_url character varying,
    avatar_storage_schema_version integer,
    avatar_updated_at timestamp without time zone,
    collections_url character varying,
    created_at timestamp without time zone NOT NULL,
    discoverable boolean,
    display_name character varying DEFAULT ''::character varying NOT NULL,
    domain character varying,
    feature_approval_policy integer DEFAULT 0 NOT NULL,
    featured_collection_url character varying,
    fields jsonb,
    followers_url character varying DEFAULT ''::character varying NOT NULL,
    following_url character varying DEFAULT ''::character varying NOT NULL,
    header_content_type character varying,
    header_description character varying DEFAULT ''::character varying NOT NULL,
    header_file_name character varying,
    header_file_size integer,
    header_remote_url character varying DEFAULT ''::character varying NOT NULL,
    header_storage_schema_version integer,
    header_updated_at timestamp without time zone,
    hide_collections boolean,
    id_scheme integer DEFAULT 1,
    inbox_url character varying DEFAULT ''::character varying NOT NULL,
    indexable boolean DEFAULT false NOT NULL,
    last_webfingered_at timestamp without time zone,
    locked boolean DEFAULT false NOT NULL,
    memorial boolean DEFAULT false NOT NULL,
    moved_to_account_id bigint,
    note text DEFAULT ''::text NOT NULL,
    outbox_url character varying DEFAULT ''::character varying NOT NULL,
    private_key text,
    protocol integer DEFAULT 0 NOT NULL,
    public_key text DEFAULT ''::text NOT NULL,
    requested_review_at timestamp without time zone,
    reviewed_at timestamp without time zone,
    sensitized_at timestamp without time zone,
    shared_inbox_url character varying DEFAULT ''::character varying NOT NULL,
    show_featured boolean DEFAULT true NOT NULL,
    show_media boolean DEFAULT true NOT NULL,
    show_media_replies boolean DEFAULT true NOT NULL,
    silenced_at timestamp without time zone,
    suspended_at timestamp without time zone,
    suspension_origin integer,
    trendable boolean,
    updated_at timestamp without time zone NOT NULL,
    uri character varying DEFAULT ''::character varying NOT NULL,
    url character varying,
    username character varying DEFAULT ''::character varying NOT NULL
);


--
-- Name: statuses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.statuses (
    id bigint DEFAULT public.timestamp_id('statuses'::text) NOT NULL,
    account_id bigint NOT NULL,
    application_id bigint,
    conversation_id bigint,
    created_at timestamp without time zone NOT NULL,
    deleted_at timestamp without time zone,
    edited_at timestamp without time zone,
    fetched_replies_at timestamp(6) without time zone,
    in_reply_to_account_id bigint,
    in_reply_to_id bigint,
    language character varying,
    local boolean,
    ordered_media_attachment_ids bigint[],
    poll_id bigint,
    quote_approval_policy integer DEFAULT 0 NOT NULL,
    reblog_of_id bigint,
    reply boolean DEFAULT false NOT NULL,
    sensitive boolean DEFAULT false NOT NULL,
    spoiler_text text DEFAULT ''::text NOT NULL,
    text text DEFAULT ''::text NOT NULL,
    trendable boolean,
    updated_at timestamp without time zone NOT NULL,
    uri character varying,
    url character varying,
    visibility integer DEFAULT 0 NOT NULL
);


--
-- Name: account_summaries; Type: MATERIALIZED VIEW; Schema: public; Owner: -
--

CREATE MATERIALIZED VIEW public.account_summaries AS
 SELECT accounts.id AS account_id,
    mode() WITHIN GROUP (ORDER BY t0.language) AS language,
    mode() WITHIN GROUP (ORDER BY t0.sensitive) AS sensitive
   FROM (public.accounts
     CROSS JOIN LATERAL ( SELECT statuses.account_id,
            statuses.language,
            statuses.sensitive
           FROM public.statuses
          WHERE ((statuses.account_id = accounts.id) AND (statuses.deleted_at IS NULL) AND (statuses.reblog_of_id IS NULL))
          ORDER BY statuses.id DESC
         LIMIT 20) t0)
  WHERE ((accounts.suspended_at IS NULL) AND (accounts.silenced_at IS NULL) AND (accounts.moved_to_account_id IS NULL) AND (accounts.discoverable = true) AND (accounts.locked = false))
  GROUP BY accounts.id
  WITH NO DATA;


--
-- Name: account_warning_presets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_warning_presets (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    text text DEFAULT ''::text NOT NULL,
    title character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_warning_presets_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_warning_presets_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_warning_presets_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_warning_presets_id_seq OWNED BY public.account_warning_presets.id;


--
-- Name: account_warnings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_warnings (
    id bigint NOT NULL,
    account_id bigint,
    action integer DEFAULT 0 NOT NULL,
    created_at timestamp without time zone NOT NULL,
    overruled_at timestamp without time zone,
    report_id bigint,
    status_ids character varying[],
    target_account_id bigint,
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: account_warnings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.account_warnings_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: account_warnings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.account_warnings_id_seq OWNED BY public.account_warnings.id;


--
-- Name: accounts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.accounts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: accounts_tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.accounts_tags (
    account_id bigint NOT NULL,
    tag_id bigint NOT NULL
);


--
-- Name: admin_action_logs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.admin_action_logs (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    action character varying DEFAULT ''::character varying NOT NULL,
    created_at timestamp without time zone NOT NULL,
    human_identifier character varying,
    permalink character varying,
    route_param character varying,
    target_id bigint,
    target_type character varying,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: admin_action_logs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.admin_action_logs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: admin_action_logs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.admin_action_logs_id_seq OWNED BY public.admin_action_logs.id;


--
-- Name: announcement_mutes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.announcement_mutes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    announcement_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: announcement_mutes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.announcement_mutes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: announcement_mutes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.announcement_mutes_id_seq OWNED BY public.announcement_mutes.id;


--
-- Name: announcement_reactions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.announcement_reactions (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    announcement_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    custom_emoji_id bigint,
    name character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: announcement_reactions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.announcement_reactions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: announcement_reactions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.announcement_reactions_id_seq OWNED BY public.announcement_reactions.id;


--
-- Name: announcements; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.announcements (
    id bigint NOT NULL,
    all_day boolean DEFAULT false NOT NULL,
    created_at timestamp without time zone NOT NULL,
    ends_at timestamp without time zone,
    notification_sent_at timestamp(6) without time zone,
    published boolean DEFAULT false NOT NULL,
    published_at timestamp without time zone,
    scheduled_at timestamp without time zone,
    starts_at timestamp without time zone,
    status_ids bigint[],
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: announcements_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.announcements_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: announcements_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.announcements_id_seq OWNED BY public.announcements.id;


--
-- Name: annual_report_statuses_per_account_counts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.annual_report_statuses_per_account_counts (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    statuses_count bigint NOT NULL,
    year integer NOT NULL
);


--
-- Name: annual_report_statuses_per_account_counts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.annual_report_statuses_per_account_counts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: annual_report_statuses_per_account_counts_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.annual_report_statuses_per_account_counts_id_seq OWNED BY public.annual_report_statuses_per_account_counts.id;


--
-- Name: appeals; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.appeals (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    account_warning_id bigint NOT NULL,
    approved_at timestamp without time zone,
    approved_by_account_id bigint,
    created_at timestamp(6) without time zone NOT NULL,
    rejected_at timestamp without time zone,
    rejected_by_account_id bigint,
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: appeals_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.appeals_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: appeals_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.appeals_id_seq OWNED BY public.appeals.id;


--
-- Name: ar_internal_metadata; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ar_internal_metadata (
    key character varying NOT NULL,
    value character varying,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: backups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.backups (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    dump_content_type character varying,
    dump_file_name character varying,
    dump_file_size bigint,
    dump_updated_at timestamp without time zone,
    processed boolean DEFAULT false NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint
);


--
-- Name: backups_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.backups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: backups_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.backups_id_seq OWNED BY public.backups.id;


--
-- Name: blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.blocks (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying
);


--
-- Name: blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.blocks_id_seq OWNED BY public.blocks.id;


--
-- Name: bookmarks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bookmarks (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: bookmarks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.bookmarks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: bookmarks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.bookmarks_id_seq OWNED BY public.bookmarks.id;


--
-- Name: bulk_import_rows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bulk_import_rows (
    id bigint NOT NULL,
    bulk_import_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    data jsonb,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: bulk_import_rows_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.bulk_import_rows_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: bulk_import_rows_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.bulk_import_rows_id_seq OWNED BY public.bulk_import_rows.id;


--
-- Name: bulk_imports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bulk_imports (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    finished_at timestamp without time zone,
    imported_items integer DEFAULT 0 NOT NULL,
    likely_mismatched boolean DEFAULT false NOT NULL,
    missing_status boolean DEFAULT false NOT NULL,
    original_filename character varying DEFAULT ''::character varying NOT NULL,
    overwrite boolean DEFAULT false NOT NULL,
    processed_items integer DEFAULT 0 NOT NULL,
    state integer NOT NULL,
    total_items integer DEFAULT 0 NOT NULL,
    type integer NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: bulk_imports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.bulk_imports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: bulk_imports_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.bulk_imports_id_seq OWNED BY public.bulk_imports.id;


--
-- Name: canonical_email_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.canonical_email_blocks (
    id bigint NOT NULL,
    canonical_email_hash character varying DEFAULT ''::character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    reference_account_id bigint,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: canonical_email_blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.canonical_email_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: canonical_email_blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.canonical_email_blocks_id_seq OWNED BY public.canonical_email_blocks.id;


--
-- Name: collection_items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.collection_items (
    id bigint DEFAULT public.timestamp_id('collection_items'::text) NOT NULL,
    account_id bigint,
    activity_uri character varying,
    approval_last_verified_at timestamp(6) without time zone,
    approval_uri character varying,
    collection_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    object_uri character varying,
    "position" integer DEFAULT 1 NOT NULL,
    state integer DEFAULT 0 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    uri character varying
);


--
-- Name: collection_items_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.collection_items_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: collection_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.collection_reports (
    id bigint NOT NULL,
    collection_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    report_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: collection_reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.collection_reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: collection_reports_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.collection_reports_id_seq OWNED BY public.collection_reports.id;


--
-- Name: collections; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.collections (
    id bigint DEFAULT public.timestamp_id('collections'::text) NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    description text,
    description_html text,
    discoverable boolean NOT NULL,
    item_count integer DEFAULT 0 NOT NULL,
    language character varying,
    local boolean NOT NULL,
    name character varying NOT NULL,
    original_number_of_items integer,
    sensitive boolean NOT NULL,
    tag_id bigint,
    updated_at timestamp(6) without time zone NOT NULL,
    uri character varying,
    url character varying
);


--
-- Name: collections_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.collections_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: conversation_mutes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversation_mutes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    conversation_id bigint NOT NULL
);


--
-- Name: conversation_mutes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.conversation_mutes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: conversation_mutes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.conversation_mutes_id_seq OWNED BY public.conversation_mutes.id;


--
-- Name: conversations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversations (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    parent_account_id bigint,
    parent_status_id bigint,
    updated_at timestamp without time zone NOT NULL,
    uri character varying
);


--
-- Name: conversations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.conversations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: conversations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.conversations_id_seq OWNED BY public.conversations.id;


--
-- Name: custom_emoji_categories; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_emoji_categories (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    featured_emoji_id bigint,
    name character varying,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: custom_emoji_categories_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.custom_emoji_categories_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: custom_emoji_categories_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.custom_emoji_categories_id_seq OWNED BY public.custom_emoji_categories.id;


--
-- Name: custom_emojis; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_emojis (
    id bigint NOT NULL,
    category_id bigint,
    created_at timestamp without time zone NOT NULL,
    disabled boolean DEFAULT false NOT NULL,
    domain character varying,
    image_content_type character varying,
    image_file_name character varying,
    image_file_size integer,
    image_remote_url character varying,
    image_storage_schema_version integer,
    image_updated_at timestamp without time zone,
    shortcode character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying,
    visible_in_picker boolean DEFAULT true NOT NULL
);


--
-- Name: custom_emojis_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.custom_emojis_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: custom_emojis_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.custom_emojis_id_seq OWNED BY public.custom_emojis.id;


--
-- Name: custom_filter_keywords; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_filter_keywords (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    custom_filter_id bigint NOT NULL,
    keyword text DEFAULT ''::text NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    whole_word boolean DEFAULT true NOT NULL
);


--
-- Name: custom_filter_keywords_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.custom_filter_keywords_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: custom_filter_keywords_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.custom_filter_keywords_id_seq OWNED BY public.custom_filter_keywords.id;


--
-- Name: custom_filter_statuses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_filter_statuses (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    custom_filter_id bigint NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: custom_filter_statuses_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.custom_filter_statuses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: custom_filter_statuses_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.custom_filter_statuses_id_seq OWNED BY public.custom_filter_statuses.id;


--
-- Name: custom_filters; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_filters (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    action integer DEFAULT 0 NOT NULL,
    context character varying[] DEFAULT '{}'::character varying[] NOT NULL,
    created_at timestamp without time zone NOT NULL,
    expires_at timestamp without time zone,
    phrase text DEFAULT ''::text NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: custom_filters_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.custom_filters_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: custom_filters_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.custom_filters_id_seq OWNED BY public.custom_filters.id;


--
-- Name: domain_allows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.domain_allows (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    domain character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: domain_allows_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.domain_allows_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: domain_allows_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.domain_allows_id_seq OWNED BY public.domain_allows.id;


--
-- Name: domain_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.domain_blocks (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    domain character varying DEFAULT ''::character varying NOT NULL,
    obfuscate boolean DEFAULT false NOT NULL,
    private_comment text,
    public_comment text,
    reject_media boolean DEFAULT false NOT NULL,
    reject_reports boolean DEFAULT false NOT NULL,
    severity integer DEFAULT 0,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: domain_blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.domain_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: domain_blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.domain_blocks_id_seq OWNED BY public.domain_blocks.id;


--
-- Name: email_domain_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.email_domain_blocks (
    id bigint NOT NULL,
    allow_with_approval boolean DEFAULT false NOT NULL,
    created_at timestamp without time zone NOT NULL,
    domain character varying DEFAULT ''::character varying NOT NULL,
    parent_id bigint,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: email_domain_blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.email_domain_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: email_domain_blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.email_domain_blocks_id_seq OWNED BY public.email_domain_blocks.id;


--
-- Name: email_subscriptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.email_subscriptions (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    confirmation_token character varying,
    confirmed_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone NOT NULL,
    email character varying NOT NULL,
    locale character varying NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: email_subscriptions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.email_subscriptions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: email_subscriptions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.email_subscriptions_id_seq OWNED BY public.email_subscriptions.id;


--
-- Name: fasp_backfill_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fasp_backfill_requests (
    id bigint NOT NULL,
    category character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    cursor character varying,
    fasp_provider_id bigint NOT NULL,
    fulfilled boolean DEFAULT false NOT NULL,
    max_count integer DEFAULT 100 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: fasp_backfill_requests_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.fasp_backfill_requests_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: fasp_backfill_requests_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.fasp_backfill_requests_id_seq OWNED BY public.fasp_backfill_requests.id;


--
-- Name: fasp_debug_callbacks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fasp_debug_callbacks (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    fasp_provider_id bigint NOT NULL,
    ip character varying NOT NULL,
    request_body text NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: fasp_debug_callbacks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.fasp_debug_callbacks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: fasp_debug_callbacks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.fasp_debug_callbacks_id_seq OWNED BY public.fasp_debug_callbacks.id;


--
-- Name: fasp_follow_recommendations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fasp_follow_recommendations (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    recommended_account_id bigint NOT NULL,
    requesting_account_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: fasp_follow_recommendations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.fasp_follow_recommendations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: fasp_follow_recommendations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.fasp_follow_recommendations_id_seq OWNED BY public.fasp_follow_recommendations.id;


--
-- Name: fasp_providers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fasp_providers (
    id bigint NOT NULL,
    base_url character varying NOT NULL,
    capabilities jsonb DEFAULT '[]'::jsonb NOT NULL,
    confirmed boolean DEFAULT false NOT NULL,
    contact_email character varying,
    created_at timestamp(6) without time zone NOT NULL,
    delivery_last_failed_at timestamp(6) without time zone,
    fediverse_account character varying,
    name character varying NOT NULL,
    privacy_policy jsonb,
    provider_public_key_pem character varying NOT NULL,
    remote_identifier character varying NOT NULL,
    server_private_key_pem character varying NOT NULL,
    sign_in_url character varying,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: fasp_providers_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.fasp_providers_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: fasp_providers_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.fasp_providers_id_seq OWNED BY public.fasp_providers.id;


--
-- Name: fasp_subscriptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fasp_subscriptions (
    id bigint NOT NULL,
    category character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    fasp_provider_id bigint NOT NULL,
    max_batch_size integer NOT NULL,
    subscription_type character varying NOT NULL,
    threshold_likes integer,
    threshold_replies integer,
    threshold_shares integer,
    threshold_timeframe integer,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: fasp_subscriptions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.fasp_subscriptions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: fasp_subscriptions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.fasp_subscriptions_id_seq OWNED BY public.fasp_subscriptions.id;


--
-- Name: favourites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.favourites (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: favourites_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.favourites_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: favourites_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.favourites_id_seq OWNED BY public.favourites.id;


--
-- Name: featured_tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.featured_tags (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    last_status_at timestamp without time zone,
    name character varying,
    statuses_count bigint DEFAULT 0 NOT NULL,
    tag_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: featured_tags_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.featured_tags_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: featured_tags_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.featured_tags_id_seq OWNED BY public.featured_tags.id;


--
-- Name: follow_recommendation_mutes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.follow_recommendation_mutes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: follow_recommendation_mutes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.follow_recommendation_mutes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: follow_recommendation_mutes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.follow_recommendation_mutes_id_seq OWNED BY public.follow_recommendation_mutes.id;


--
-- Name: follow_recommendation_suppressions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.follow_recommendation_suppressions (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: follow_recommendation_suppressions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.follow_recommendation_suppressions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: follow_recommendation_suppressions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.follow_recommendation_suppressions_id_seq OWNED BY public.follow_recommendation_suppressions.id;


--
-- Name: follow_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.follow_requests (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    languages character varying[],
    notify boolean DEFAULT false NOT NULL,
    show_reblogs boolean DEFAULT true NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying
);


--
-- Name: follow_requests_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.follow_requests_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: follow_requests_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.follow_requests_id_seq OWNED BY public.follow_requests.id;


--
-- Name: follows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.follows (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    languages character varying[],
    notify boolean DEFAULT false NOT NULL,
    show_reblogs boolean DEFAULT true NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying
);


--
-- Name: follows_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.follows_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: follows_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.follows_id_seq OWNED BY public.follows.id;


--
-- Name: generated_annual_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.generated_annual_reports (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    data jsonb NOT NULL,
    schema_version integer NOT NULL,
    share_key character varying,
    updated_at timestamp(6) without time zone NOT NULL,
    viewed_at timestamp(6) without time zone,
    year integer NOT NULL
);


--
-- Name: generated_annual_reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.generated_annual_reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: generated_annual_reports_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.generated_annual_reports_id_seq OWNED BY public.generated_annual_reports.id;


--
-- Name: status_stats; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.status_stats (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    favourites_count bigint DEFAULT 0 NOT NULL,
    quotes_count bigint DEFAULT 0 NOT NULL,
    reblogs_count bigint DEFAULT 0 NOT NULL,
    replies_count bigint DEFAULT 0 NOT NULL,
    status_id bigint NOT NULL,
    untrusted_favourites_count bigint,
    untrusted_reblogs_count bigint,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    age_verified_at timestamp(6) without time zone,
    approved boolean DEFAULT true NOT NULL,
    chosen_languages character varying[],
    confirmation_sent_at timestamp without time zone,
    confirmation_token character varying,
    confirmed_at timestamp without time zone,
    consumed_timestep integer,
    created_at timestamp without time zone NOT NULL,
    created_by_application_id bigint,
    current_sign_in_at timestamp without time zone,
    disabled boolean DEFAULT false NOT NULL,
    email character varying DEFAULT ''::character varying NOT NULL,
    encrypted_password character varying DEFAULT ''::character varying NOT NULL,
    invite_id bigint,
    last_emailed_at timestamp without time zone,
    last_sign_in_at timestamp without time zone,
    locale character varying,
    otp_backup_codes character varying[],
    otp_required_for_login boolean DEFAULT false NOT NULL,
    otp_secret character varying,
    require_tos_interstitial boolean DEFAULT false NOT NULL,
    reset_password_sent_at timestamp without time zone,
    reset_password_token character varying,
    role_id bigint,
    settings text,
    sign_in_count integer DEFAULT 0 NOT NULL,
    sign_in_token character varying,
    sign_in_token_sent_at timestamp without time zone,
    sign_up_ip inet,
    skip_sign_in_token boolean,
    time_zone character varying,
    unconfirmed_email character varying,
    updated_at timestamp without time zone NOT NULL,
    webauthn_id character varying
);


--
-- Name: global_follow_recommendations; Type: MATERIALIZED VIEW; Schema: public; Owner: -
--

CREATE MATERIALIZED VIEW public.global_follow_recommendations AS
 SELECT t0.account_id,
    sum(t0.rank) AS rank,
    array_agg(t0.reason) AS reason
   FROM ( SELECT account_summaries.account_id,
            ((count(follows.id))::numeric / (1.0 + (count(follows.id))::numeric)) AS rank,
            'most_followed'::text AS reason
           FROM ((public.follows
             JOIN public.account_summaries ON ((account_summaries.account_id = follows.target_account_id)))
             JOIN public.users ON ((users.account_id = follows.account_id)))
          WHERE ((users.current_sign_in_at >= (now() - '30 days'::interval)) AND (account_summaries.sensitive = false) AND (NOT (EXISTS ( SELECT 1
                   FROM public.follow_recommendation_suppressions
                  WHERE (follow_recommendation_suppressions.account_id = follows.target_account_id)))))
          GROUP BY account_summaries.account_id
         HAVING (count(follows.id) >= 5)
        UNION ALL
         SELECT account_summaries.account_id,
            (sum((status_stats.reblogs_count + status_stats.favourites_count)) / (1.0 + sum((status_stats.reblogs_count + status_stats.favourites_count)))) AS rank,
            'most_interactions'::text AS reason
           FROM ((public.status_stats
             JOIN public.statuses ON ((statuses.id = status_stats.status_id)))
             JOIN public.account_summaries ON ((account_summaries.account_id = statuses.account_id)))
          WHERE ((statuses.id >= (((date_part('epoch'::text, (now() - '30 days'::interval)) * (1000)::double precision))::bigint << 16)) AND (account_summaries.sensitive = false) AND (NOT (EXISTS ( SELECT 1
                   FROM public.follow_recommendation_suppressions
                  WHERE (follow_recommendation_suppressions.account_id = statuses.account_id)))))
          GROUP BY account_summaries.account_id
         HAVING (sum((status_stats.reblogs_count + status_stats.favourites_count)) >= (5)::numeric)) t0
  GROUP BY t0.account_id
  ORDER BY (sum(t0.rank)) DESC
  WITH NO DATA;


--
-- Name: identities; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.identities (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    provider character varying DEFAULT ''::character varying NOT NULL,
    uid character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint
);


--
-- Name: identities_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.identities_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: identities_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.identities_id_seq OWNED BY public.identities.id;


--
-- Name: instance_moderation_notes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.instance_moderation_notes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    content text,
    created_at timestamp(6) without time zone NOT NULL,
    domain character varying NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: instance_moderation_notes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.instance_moderation_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: instance_moderation_notes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.instance_moderation_notes_id_seq OWNED BY public.instance_moderation_notes.id;


--
-- Name: instances; Type: MATERIALIZED VIEW; Schema: public; Owner: -
--

CREATE MATERIALIZED VIEW public.instances AS
 WITH domain_counts(domain, accounts_count) AS (
         SELECT accounts.domain,
            count(*) AS accounts_count
           FROM public.accounts
          WHERE (accounts.domain IS NOT NULL)
          GROUP BY accounts.domain
        )
 SELECT domain_counts.domain,
    domain_counts.accounts_count
   FROM domain_counts
UNION
 SELECT domain_blocks.domain,
    COALESCE(domain_counts.accounts_count, (0)::bigint) AS accounts_count
   FROM (public.domain_blocks
     LEFT JOIN domain_counts ON (((domain_counts.domain)::text = (domain_blocks.domain)::text)))
UNION
 SELECT domain_allows.domain,
    COALESCE(domain_counts.accounts_count, (0)::bigint) AS accounts_count
   FROM (public.domain_allows
     LEFT JOIN domain_counts ON (((domain_counts.domain)::text = (domain_allows.domain)::text)))
  WITH NO DATA;


--
-- Name: invites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invites (
    id bigint NOT NULL,
    autofollow boolean DEFAULT false NOT NULL,
    code character varying DEFAULT ''::character varying NOT NULL,
    comment text,
    created_at timestamp without time zone NOT NULL,
    expires_at timestamp without time zone,
    max_uses integer,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint NOT NULL,
    uses integer DEFAULT 0 NOT NULL
);


--
-- Name: invites_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.invites_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: invites_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.invites_id_seq OWNED BY public.invites.id;


--
-- Name: ip_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ip_blocks (
    id bigint NOT NULL,
    comment text DEFAULT ''::text NOT NULL,
    created_at timestamp without time zone NOT NULL,
    expires_at timestamp without time zone,
    ip inet DEFAULT '0.0.0.0'::inet NOT NULL,
    severity integer DEFAULT 0 NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: ip_blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.ip_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: ip_blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.ip_blocks_id_seq OWNED BY public.ip_blocks.id;


--
-- Name: keypairs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.keypairs (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    expires_at timestamp(6) without time zone,
    private_key character varying,
    public_key character varying NOT NULL,
    revoked boolean DEFAULT false NOT NULL,
    type integer NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    uri character varying NOT NULL
);


--
-- Name: keypairs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.keypairs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: keypairs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.keypairs_id_seq OWNED BY public.keypairs.id;


--
-- Name: list_accounts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.list_accounts (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    follow_id bigint,
    follow_request_id bigint,
    list_id bigint NOT NULL
);


--
-- Name: list_accounts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.list_accounts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: list_accounts_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.list_accounts_id_seq OWNED BY public.list_accounts.id;


--
-- Name: lists; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.lists (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    exclusive boolean DEFAULT false NOT NULL,
    replies_policy integer DEFAULT 0 NOT NULL,
    title character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: lists_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.lists_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: lists_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.lists_id_seq OWNED BY public.lists.id;


--
-- Name: login_activities; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.login_activities (
    id bigint NOT NULL,
    authentication_method character varying,
    created_at timestamp without time zone,
    failure_reason character varying,
    ip inet,
    provider character varying,
    success boolean,
    user_agent character varying,
    user_id bigint NOT NULL
);


--
-- Name: login_activities_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.login_activities_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: login_activities_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.login_activities_id_seq OWNED BY public.login_activities.id;


--
-- Name: markers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.markers (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    last_read_id bigint DEFAULT 0 NOT NULL,
    lock_version integer DEFAULT 0 NOT NULL,
    timeline character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint NOT NULL
);


--
-- Name: markers_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.markers_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: markers_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.markers_id_seq OWNED BY public.markers.id;


--
-- Name: media_attachments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.media_attachments (
    id bigint DEFAULT public.timestamp_id('media_attachments'::text) NOT NULL,
    account_id bigint,
    blurhash character varying,
    created_at timestamp without time zone NOT NULL,
    description text,
    file_content_type character varying,
    file_file_name character varying,
    file_file_size integer,
    file_meta json,
    file_storage_schema_version integer,
    file_updated_at timestamp without time zone,
    processing integer,
    remote_url character varying DEFAULT ''::character varying NOT NULL,
    scheduled_status_id bigint,
    shortcode character varying,
    status_id bigint,
    thumbnail_content_type character varying,
    thumbnail_file_name character varying,
    thumbnail_file_size integer,
    thumbnail_remote_url character varying,
    thumbnail_storage_schema_version integer,
    thumbnail_updated_at timestamp without time zone,
    type integer DEFAULT 0 NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: media_attachments_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.media_attachments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: mentions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mentions (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    silent boolean DEFAULT false NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: mentions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.mentions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: mentions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.mentions_id_seq OWNED BY public.mentions.id;


--
-- Name: mutes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mutes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    expires_at timestamp without time zone,
    hide_notifications boolean DEFAULT true NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: mutes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.mutes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: mutes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.mutes_id_seq OWNED BY public.mutes.id;


--
-- Name: notification_permissions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notification_permissions (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    from_account_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: notification_permissions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.notification_permissions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: notification_permissions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.notification_permissions_id_seq OWNED BY public.notification_permissions.id;


--
-- Name: notification_policies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notification_policies (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    for_bots integer DEFAULT 0 NOT NULL,
    for_limited_accounts integer DEFAULT 1 NOT NULL,
    for_new_accounts integer DEFAULT 0 NOT NULL,
    for_not_followers integer DEFAULT 0 NOT NULL,
    for_not_following integer DEFAULT 0 NOT NULL,
    for_private_mentions integer DEFAULT 1 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: notification_policies_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.notification_policies_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: notification_policies_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.notification_policies_id_seq OWNED BY public.notification_policies.id;


--
-- Name: notification_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notification_requests (
    id bigint DEFAULT public.timestamp_id('notification_requests'::text) NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    from_account_id bigint NOT NULL,
    last_status_id bigint,
    notifications_count bigint DEFAULT 0 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: notification_requests_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.notification_requests_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notifications (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    activity_id bigint NOT NULL,
    activity_type character varying NOT NULL,
    created_at timestamp without time zone NOT NULL,
    filtered boolean DEFAULT false NOT NULL,
    from_account_id bigint NOT NULL,
    group_key character varying,
    type character varying,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: notifications_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.notifications_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: notifications_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.notifications_id_seq OWNED BY public.notifications.id;


--
-- Name: oauth_access_grants; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.oauth_access_grants (
    id bigint NOT NULL,
    application_id bigint NOT NULL,
    code_challenge character varying,
    code_challenge_method character varying,
    created_at timestamp without time zone NOT NULL,
    expires_in integer NOT NULL,
    redirect_uri text NOT NULL,
    resource_owner_id bigint NOT NULL,
    revoked_at timestamp without time zone,
    scopes character varying,
    token character varying NOT NULL
);


--
-- Name: oauth_access_grants_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.oauth_access_grants_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: oauth_access_grants_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.oauth_access_grants_id_seq OWNED BY public.oauth_access_grants.id;


--
-- Name: oauth_access_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.oauth_access_tokens (
    id bigint NOT NULL,
    application_id bigint,
    created_at timestamp without time zone NOT NULL,
    expires_in integer,
    last_used_at timestamp without time zone,
    last_used_ip inet,
    refresh_token character varying,
    resource_owner_id bigint,
    revoked_at timestamp without time zone,
    scopes character varying,
    token character varying NOT NULL
);


--
-- Name: oauth_access_tokens_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.oauth_access_tokens_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: oauth_access_tokens_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.oauth_access_tokens_id_seq OWNED BY public.oauth_access_tokens.id;


--
-- Name: oauth_applications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.oauth_applications (
    id bigint NOT NULL,
    confidential boolean DEFAULT true NOT NULL,
    created_at timestamp without time zone,
    name character varying NOT NULL,
    owner_id bigint,
    owner_type character varying,
    redirect_uri text NOT NULL,
    scopes character varying DEFAULT ''::character varying NOT NULL,
    secret character varying NOT NULL,
    superapp boolean DEFAULT false NOT NULL,
    uid character varying NOT NULL,
    updated_at timestamp without time zone,
    website character varying
);


--
-- Name: oauth_applications_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.oauth_applications_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: oauth_applications_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.oauth_applications_id_seq OWNED BY public.oauth_applications.id;


--
-- Name: pghero_space_stats; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.pghero_space_stats (
    id bigint NOT NULL,
    captured_at timestamp without time zone,
    database text,
    relation text,
    schema text,
    size bigint
);


--
-- Name: pghero_space_stats_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.pghero_space_stats_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: pghero_space_stats_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.pghero_space_stats_id_seq OWNED BY public.pghero_space_stats.id;


--
-- Name: poll_votes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.poll_votes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    choice integer DEFAULT 0 NOT NULL,
    created_at timestamp without time zone NOT NULL,
    poll_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying
);


--
-- Name: poll_votes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.poll_votes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: poll_votes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.poll_votes_id_seq OWNED BY public.poll_votes.id;


--
-- Name: polls; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.polls (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    cached_tallies bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    created_at timestamp without time zone NOT NULL,
    expires_at timestamp without time zone,
    hide_totals boolean DEFAULT false NOT NULL,
    last_fetched_at timestamp without time zone,
    lock_version integer DEFAULT 0 NOT NULL,
    multiple boolean DEFAULT false NOT NULL,
    options character varying[] DEFAULT '{}'::character varying[] NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    voters_count bigint,
    votes_count bigint DEFAULT 0 NOT NULL
);


--
-- Name: polls_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.polls_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: polls_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.polls_id_seq OWNED BY public.polls.id;


--
-- Name: preview_card_providers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.preview_card_providers (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    domain character varying DEFAULT ''::character varying NOT NULL,
    icon_content_type character varying,
    icon_file_name character varying,
    icon_file_size bigint,
    icon_updated_at timestamp without time zone,
    requested_review_at timestamp without time zone,
    reviewed_at timestamp without time zone,
    trendable boolean,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: preview_card_providers_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.preview_card_providers_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: preview_card_providers_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.preview_card_providers_id_seq OWNED BY public.preview_card_providers.id;


--
-- Name: preview_card_trends; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.preview_card_trends (
    id bigint NOT NULL,
    allowed boolean DEFAULT false NOT NULL,
    language character varying,
    preview_card_id bigint NOT NULL,
    rank integer DEFAULT 0 NOT NULL,
    score double precision DEFAULT 0.0 NOT NULL
);


--
-- Name: preview_card_trends_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.preview_card_trends_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: preview_card_trends_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.preview_card_trends_id_seq OWNED BY public.preview_card_trends.id;


--
-- Name: preview_cards; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.preview_cards (
    id bigint NOT NULL,
    author_account_id bigint,
    author_name character varying DEFAULT ''::character varying NOT NULL,
    author_url character varying DEFAULT ''::character varying NOT NULL,
    blurhash character varying,
    created_at timestamp without time zone NOT NULL,
    description character varying DEFAULT ''::character varying NOT NULL,
    embed_url character varying DEFAULT ''::character varying NOT NULL,
    height integer DEFAULT 0 NOT NULL,
    html text DEFAULT ''::text NOT NULL,
    image_content_type character varying,
    image_description character varying DEFAULT ''::character varying NOT NULL,
    image_file_name character varying,
    image_file_size integer,
    image_storage_schema_version integer,
    image_updated_at timestamp without time zone,
    language character varying,
    link_type integer,
    max_score double precision,
    max_score_at timestamp without time zone,
    provider_name character varying DEFAULT ''::character varying NOT NULL,
    provider_url character varying DEFAULT ''::character varying NOT NULL,
    published_at timestamp(6) without time zone,
    title character varying DEFAULT ''::character varying NOT NULL,
    trendable boolean,
    type integer DEFAULT 0 NOT NULL,
    unverified_author_account_id bigint,
    updated_at timestamp without time zone NOT NULL,
    url character varying DEFAULT ''::character varying NOT NULL,
    width integer DEFAULT 0 NOT NULL
);


--
-- Name: preview_cards_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.preview_cards_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: preview_cards_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.preview_cards_id_seq OWNED BY public.preview_cards.id;


--
-- Name: preview_cards_statuses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.preview_cards_statuses (
    preview_card_id bigint NOT NULL,
    status_id bigint NOT NULL,
    url character varying
);


--
-- Name: quotes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.quotes (
    id bigint DEFAULT public.timestamp_id('quotes'::text) NOT NULL,
    account_id bigint NOT NULL,
    activity_uri character varying,
    approval_uri character varying,
    created_at timestamp(6) without time zone NOT NULL,
    legacy boolean DEFAULT false NOT NULL,
    quoted_account_id bigint,
    quoted_status_id bigint,
    state integer DEFAULT 0 NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: quotes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.quotes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: relationship_severance_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.relationship_severance_events (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    purged boolean DEFAULT false NOT NULL,
    target_name character varying NOT NULL,
    type integer NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: relationship_severance_events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.relationship_severance_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: relationship_severance_events_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.relationship_severance_events_id_seq OWNED BY public.relationship_severance_events.id;


--
-- Name: relays; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.relays (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    follow_activity_id character varying,
    inbox_url character varying DEFAULT ''::character varying NOT NULL,
    state integer DEFAULT 0 NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: relays_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.relays_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: relays_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.relays_id_seq OWNED BY public.relays.id;


--
-- Name: report_notes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.report_notes (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    content text NOT NULL,
    created_at timestamp without time zone NOT NULL,
    report_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: report_notes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.report_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: report_notes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.report_notes_id_seq OWNED BY public.report_notes.id;


--
-- Name: reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reports (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    action_taken_at timestamp without time zone,
    action_taken_by_account_id bigint,
    application_id bigint,
    assigned_account_id bigint,
    category integer DEFAULT 0 NOT NULL,
    comment text DEFAULT ''::text NOT NULL,
    created_at timestamp without time zone NOT NULL,
    forwarded boolean,
    rule_ids bigint[],
    status_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    target_account_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying
);


--
-- Name: reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: reports_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.reports_id_seq OWNED BY public.reports.id;


--
-- Name: rule_translations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rule_translations (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    hint text DEFAULT ''::text NOT NULL,
    language character varying NOT NULL,
    rule_id bigint NOT NULL,
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: rule_translations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.rule_translations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: rule_translations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.rule_translations_id_seq OWNED BY public.rule_translations.id;


--
-- Name: rules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rules (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    deleted_at timestamp without time zone,
    hint text DEFAULT ''::text NOT NULL,
    priority integer DEFAULT 0 NOT NULL,
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: rules_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.rules_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: rules_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.rules_id_seq OWNED BY public.rules.id;


--
-- Name: scheduled_statuses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.scheduled_statuses (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    params jsonb,
    scheduled_at timestamp without time zone
);


--
-- Name: scheduled_statuses_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.scheduled_statuses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: scheduled_statuses_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.scheduled_statuses_id_seq OWNED BY public.scheduled_statuses.id;


--
-- Name: schema_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.schema_migrations (
    version character varying NOT NULL
);


--
-- Name: session_activations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.session_activations (
    id bigint NOT NULL,
    access_token_id bigint,
    created_at timestamp without time zone NOT NULL,
    ip inet,
    session_id character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    user_agent character varying DEFAULT ''::character varying NOT NULL,
    user_id bigint NOT NULL,
    web_push_subscription_id bigint
);


--
-- Name: session_activations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.session_activations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: session_activations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.session_activations_id_seq OWNED BY public.session_activations.id;


--
-- Name: settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.settings (
    id bigint NOT NULL,
    created_at timestamp without time zone,
    updated_at timestamp without time zone,
    value text,
    var character varying NOT NULL
);


--
-- Name: settings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.settings_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: settings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.settings_id_seq OWNED BY public.settings.id;


--
-- Name: severed_relationships; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.severed_relationships (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    direction integer NOT NULL,
    languages character varying[],
    local_account_id bigint NOT NULL,
    notify boolean,
    relationship_severance_event_id bigint NOT NULL,
    remote_account_id bigint NOT NULL,
    show_reblogs boolean,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: severed_relationships_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.severed_relationships_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: severed_relationships_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.severed_relationships_id_seq OWNED BY public.severed_relationships.id;


--
-- Name: site_uploads; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.site_uploads (
    id bigint NOT NULL,
    blurhash character varying,
    created_at timestamp without time zone NOT NULL,
    file_content_type character varying,
    file_file_name character varying,
    file_file_size integer,
    file_updated_at timestamp without time zone,
    meta json,
    updated_at timestamp without time zone NOT NULL,
    var character varying DEFAULT ''::character varying NOT NULL
);


--
-- Name: site_uploads_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.site_uploads_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: site_uploads_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.site_uploads_id_seq OWNED BY public.site_uploads.id;


--
-- Name: software_updates; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.software_updates (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    release_notes character varying DEFAULT ''::character varying NOT NULL,
    type integer DEFAULT 0 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    urgent boolean DEFAULT false NOT NULL,
    version character varying NOT NULL
);


--
-- Name: software_updates_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.software_updates_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: software_updates_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.software_updates_id_seq OWNED BY public.software_updates.id;


--
-- Name: status_edits; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.status_edits (
    id bigint NOT NULL,
    account_id bigint,
    created_at timestamp(6) without time zone NOT NULL,
    media_descriptions text[],
    ordered_media_attachment_ids bigint[],
    poll_options character varying[],
    quote_id bigint,
    sensitive boolean,
    spoiler_text text DEFAULT ''::text NOT NULL,
    status_id bigint NOT NULL,
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: status_edits_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.status_edits_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: status_edits_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.status_edits_id_seq OWNED BY public.status_edits.id;


--
-- Name: status_pins; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.status_pins (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    status_id bigint NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: status_pins_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.status_pins_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: status_pins_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.status_pins_id_seq OWNED BY public.status_pins.id;


--
-- Name: status_stats_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.status_stats_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: status_stats_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.status_stats_id_seq OWNED BY public.status_stats.id;


--
-- Name: status_trends; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.status_trends (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    allowed boolean DEFAULT false NOT NULL,
    language character varying,
    rank integer DEFAULT 0 NOT NULL,
    score double precision DEFAULT 0.0 NOT NULL,
    status_id bigint NOT NULL
);


--
-- Name: status_trends_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.status_trends_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: status_trends_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.status_trends_id_seq OWNED BY public.status_trends.id;


--
-- Name: statuses_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.statuses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: statuses_tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.statuses_tags (
    status_id bigint NOT NULL,
    tag_id bigint NOT NULL
);


--
-- Name: tag_follows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tag_follows (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    tag_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: tag_follows_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.tag_follows_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tag_follows_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.tag_follows_id_seq OWNED BY public.tag_follows.id;


--
-- Name: tag_trends; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tag_trends (
    id bigint NOT NULL,
    allowed boolean DEFAULT false NOT NULL,
    language character varying DEFAULT ''::character varying NOT NULL,
    rank integer DEFAULT 0 NOT NULL,
    score double precision DEFAULT 0.0 NOT NULL,
    tag_id bigint NOT NULL
);


--
-- Name: tag_trends_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.tag_trends_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tag_trends_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.tag_trends_id_seq OWNED BY public.tag_trends.id;


--
-- Name: tagged_objects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tagged_objects (
    id bigint NOT NULL,
    ap_type character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    object_id bigint,
    object_type character varying,
    status_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    uri character varying
);


--
-- Name: tagged_objects_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.tagged_objects_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tagged_objects_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.tagged_objects_id_seq OWNED BY public.tagged_objects.id;


--
-- Name: tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tags (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    display_name character varying,
    last_status_at timestamp without time zone,
    listable boolean,
    max_score double precision,
    max_score_at timestamp without time zone,
    name character varying DEFAULT ''::character varying NOT NULL,
    requested_review_at timestamp without time zone,
    reviewed_at timestamp without time zone,
    trendable boolean,
    updated_at timestamp without time zone NOT NULL,
    usable boolean
);


--
-- Name: tags_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.tags_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tags_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.tags_id_seq OWNED BY public.tags.id;


--
-- Name: terms_of_services; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.terms_of_services (
    id bigint NOT NULL,
    changelog text DEFAULT ''::text NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    effective_date date,
    notification_sent_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    text text DEFAULT ''::text NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: terms_of_services_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.terms_of_services_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: terms_of_services_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.terms_of_services_id_seq OWNED BY public.terms_of_services.id;


--
-- Name: tombstones; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tombstones (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    by_moderator boolean,
    created_at timestamp without time zone NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    uri character varying NOT NULL
);


--
-- Name: tombstones_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.tombstones_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tombstones_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.tombstones_id_seq OWNED BY public.tombstones.id;


--
-- Name: unavailable_domains; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.unavailable_domains (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    domain character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp without time zone NOT NULL
);


--
-- Name: unavailable_domains_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.unavailable_domains_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: unavailable_domains_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.unavailable_domains_id_seq OWNED BY public.unavailable_domains.id;


--
-- Name: user_invite_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_invite_requests (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    text text,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint NOT NULL
);


--
-- Name: user_invite_requests_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_invite_requests_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_invite_requests_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_invite_requests_id_seq OWNED BY public.user_invite_requests.id;


--
-- Name: user_ips; Type: VIEW; Schema: public; Owner: -
--

CREATE VIEW public.user_ips AS
 SELECT t0.user_id,
    t0.ip,
    max(t0.used_at) AS used_at
   FROM ( SELECT users.id AS user_id,
            users.sign_up_ip AS ip,
            users.created_at AS used_at
           FROM public.users
          WHERE (users.sign_up_ip IS NOT NULL)
        UNION ALL
         SELECT session_activations.user_id,
            session_activations.ip,
            session_activations.updated_at
           FROM public.session_activations
        UNION ALL
         SELECT login_activities.user_id,
            login_activities.ip,
            login_activities.created_at
           FROM public.login_activities
          WHERE (login_activities.success = true)) t0
  GROUP BY t0.user_id, t0.ip;


--
-- Name: user_roles; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_roles (
    id bigint NOT NULL,
    collection_limit integer DEFAULT 10 NOT NULL,
    color character varying DEFAULT ''::character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    highlighted boolean DEFAULT false NOT NULL,
    name character varying DEFAULT ''::character varying NOT NULL,
    permissions bigint DEFAULT 0 NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    require_2fa boolean DEFAULT false NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: user_roles_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_roles_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_roles_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_roles_id_seq OWNED BY public.user_roles.id;


--
-- Name: username_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.username_blocks (
    id bigint NOT NULL,
    allow_with_approval boolean DEFAULT false NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    exact boolean DEFAULT false NOT NULL,
    normalized_username character varying NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    username character varying NOT NULL
);


--
-- Name: username_blocks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.username_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: username_blocks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.username_blocks_id_seq OWNED BY public.username_blocks.id;


--
-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;


--
-- Name: web_push_subscriptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.web_push_subscriptions (
    id bigint NOT NULL,
    access_token_id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    data json,
    endpoint character varying NOT NULL,
    key_auth character varying NOT NULL,
    key_p256dh character varying NOT NULL,
    standard boolean DEFAULT false NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint NOT NULL
);


--
-- Name: web_push_subscriptions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.web_push_subscriptions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: web_push_subscriptions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.web_push_subscriptions_id_seq OWNED BY public.web_push_subscriptions.id;


--
-- Name: web_settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.web_settings (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    data json,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint NOT NULL
);


--
-- Name: web_settings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.web_settings_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: web_settings_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.web_settings_id_seq OWNED BY public.web_settings.id;


--
-- Name: webauthn_credentials; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webauthn_credentials (
    id bigint NOT NULL,
    created_at timestamp without time zone NOT NULL,
    external_id character varying NOT NULL,
    nickname character varying NOT NULL,
    public_key character varying NOT NULL,
    sign_count bigint DEFAULT 0 NOT NULL,
    updated_at timestamp without time zone NOT NULL,
    user_id bigint
);


--
-- Name: webauthn_credentials_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.webauthn_credentials_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: webauthn_credentials_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.webauthn_credentials_id_seq OWNED BY public.webauthn_credentials.id;


--
-- Name: webhooks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webhooks (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    events character varying[] DEFAULT '{}'::character varying[] NOT NULL,
    secret character varying DEFAULT ''::character varying NOT NULL,
    template text,
    updated_at timestamp(6) without time zone NOT NULL,
    url character varying NOT NULL
);


--
-- Name: webhooks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.webhooks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: webhooks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.webhooks_id_seq OWNED BY public.webhooks.id;


--
-- Name: account_aliases id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_aliases ALTER COLUMN id SET DEFAULT nextval('public.account_aliases_id_seq'::regclass);


--
-- Name: account_conversations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_conversations ALTER COLUMN id SET DEFAULT nextval('public.account_conversations_id_seq'::regclass);


--
-- Name: account_deletion_requests id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_deletion_requests ALTER COLUMN id SET DEFAULT nextval('public.account_deletion_requests_id_seq'::regclass);


--
-- Name: account_domain_blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_domain_blocks ALTER COLUMN id SET DEFAULT nextval('public.account_domain_blocks_id_seq'::regclass);


--
-- Name: account_migrations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_migrations ALTER COLUMN id SET DEFAULT nextval('public.account_migrations_id_seq'::regclass);


--
-- Name: account_moderation_notes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_moderation_notes ALTER COLUMN id SET DEFAULT nextval('public.account_moderation_notes_id_seq'::regclass);


--
-- Name: account_notes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_notes ALTER COLUMN id SET DEFAULT nextval('public.account_notes_id_seq'::regclass);


--
-- Name: account_pins id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_pins ALTER COLUMN id SET DEFAULT nextval('public.account_pins_id_seq'::regclass);


--
-- Name: account_relationship_severance_events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_relationship_severance_events ALTER COLUMN id SET DEFAULT nextval('public.account_relationship_severance_events_id_seq'::regclass);


--
-- Name: account_stats id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_stats ALTER COLUMN id SET DEFAULT nextval('public.account_stats_id_seq'::regclass);


--
-- Name: account_statuses_cleanup_policies id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_statuses_cleanup_policies ALTER COLUMN id SET DEFAULT nextval('public.account_statuses_cleanup_policies_id_seq'::regclass);


--
-- Name: account_warning_presets id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warning_presets ALTER COLUMN id SET DEFAULT nextval('public.account_warning_presets_id_seq'::regclass);


--
-- Name: account_warnings id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warnings ALTER COLUMN id SET DEFAULT nextval('public.account_warnings_id_seq'::regclass);


--
-- Name: admin_action_logs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_action_logs ALTER COLUMN id SET DEFAULT nextval('public.admin_action_logs_id_seq'::regclass);


--
-- Name: announcement_mutes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_mutes ALTER COLUMN id SET DEFAULT nextval('public.announcement_mutes_id_seq'::regclass);


--
-- Name: announcement_reactions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_reactions ALTER COLUMN id SET DEFAULT nextval('public.announcement_reactions_id_seq'::regclass);


--
-- Name: announcements id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcements ALTER COLUMN id SET DEFAULT nextval('public.announcements_id_seq'::regclass);


--
-- Name: annual_report_statuses_per_account_counts id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.annual_report_statuses_per_account_counts ALTER COLUMN id SET DEFAULT nextval('public.annual_report_statuses_per_account_counts_id_seq'::regclass);


--
-- Name: appeals id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appeals ALTER COLUMN id SET DEFAULT nextval('public.appeals_id_seq'::regclass);


--
-- Name: backups id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.backups ALTER COLUMN id SET DEFAULT nextval('public.backups_id_seq'::regclass);


--
-- Name: blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.blocks ALTER COLUMN id SET DEFAULT nextval('public.blocks_id_seq'::regclass);


--
-- Name: bookmarks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks ALTER COLUMN id SET DEFAULT nextval('public.bookmarks_id_seq'::regclass);


--
-- Name: bulk_import_rows id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bulk_import_rows ALTER COLUMN id SET DEFAULT nextval('public.bulk_import_rows_id_seq'::regclass);


--
-- Name: bulk_imports id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bulk_imports ALTER COLUMN id SET DEFAULT nextval('public.bulk_imports_id_seq'::regclass);


--
-- Name: canonical_email_blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.canonical_email_blocks ALTER COLUMN id SET DEFAULT nextval('public.canonical_email_blocks_id_seq'::regclass);


--
-- Name: collection_reports id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_reports ALTER COLUMN id SET DEFAULT nextval('public.collection_reports_id_seq'::regclass);


--
-- Name: conversation_mutes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_mutes ALTER COLUMN id SET DEFAULT nextval('public.conversation_mutes_id_seq'::regclass);


--
-- Name: conversations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversations ALTER COLUMN id SET DEFAULT nextval('public.conversations_id_seq'::regclass);


--
-- Name: custom_emoji_categories id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emoji_categories ALTER COLUMN id SET DEFAULT nextval('public.custom_emoji_categories_id_seq'::regclass);


--
-- Name: custom_emojis id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emojis ALTER COLUMN id SET DEFAULT nextval('public.custom_emojis_id_seq'::regclass);


--
-- Name: custom_filter_keywords id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_keywords ALTER COLUMN id SET DEFAULT nextval('public.custom_filter_keywords_id_seq'::regclass);


--
-- Name: custom_filter_statuses id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_statuses ALTER COLUMN id SET DEFAULT nextval('public.custom_filter_statuses_id_seq'::regclass);


--
-- Name: custom_filters id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filters ALTER COLUMN id SET DEFAULT nextval('public.custom_filters_id_seq'::regclass);


--
-- Name: domain_allows id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.domain_allows ALTER COLUMN id SET DEFAULT nextval('public.domain_allows_id_seq'::regclass);


--
-- Name: domain_blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.domain_blocks ALTER COLUMN id SET DEFAULT nextval('public.domain_blocks_id_seq'::regclass);


--
-- Name: email_domain_blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_domain_blocks ALTER COLUMN id SET DEFAULT nextval('public.email_domain_blocks_id_seq'::regclass);


--
-- Name: email_subscriptions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_subscriptions ALTER COLUMN id SET DEFAULT nextval('public.email_subscriptions_id_seq'::regclass);


--
-- Name: fasp_backfill_requests id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_backfill_requests ALTER COLUMN id SET DEFAULT nextval('public.fasp_backfill_requests_id_seq'::regclass);


--
-- Name: fasp_debug_callbacks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_debug_callbacks ALTER COLUMN id SET DEFAULT nextval('public.fasp_debug_callbacks_id_seq'::regclass);


--
-- Name: fasp_follow_recommendations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_follow_recommendations ALTER COLUMN id SET DEFAULT nextval('public.fasp_follow_recommendations_id_seq'::regclass);


--
-- Name: fasp_providers id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_providers ALTER COLUMN id SET DEFAULT nextval('public.fasp_providers_id_seq'::regclass);


--
-- Name: fasp_subscriptions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_subscriptions ALTER COLUMN id SET DEFAULT nextval('public.fasp_subscriptions_id_seq'::regclass);


--
-- Name: favourites id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favourites ALTER COLUMN id SET DEFAULT nextval('public.favourites_id_seq'::regclass);


--
-- Name: featured_tags id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.featured_tags ALTER COLUMN id SET DEFAULT nextval('public.featured_tags_id_seq'::regclass);


--
-- Name: follow_recommendation_mutes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_mutes ALTER COLUMN id SET DEFAULT nextval('public.follow_recommendation_mutes_id_seq'::regclass);


--
-- Name: follow_recommendation_suppressions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_suppressions ALTER COLUMN id SET DEFAULT nextval('public.follow_recommendation_suppressions_id_seq'::regclass);


--
-- Name: follow_requests id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_requests ALTER COLUMN id SET DEFAULT nextval('public.follow_requests_id_seq'::regclass);


--
-- Name: follows id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follows ALTER COLUMN id SET DEFAULT nextval('public.follows_id_seq'::regclass);


--
-- Name: generated_annual_reports id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.generated_annual_reports ALTER COLUMN id SET DEFAULT nextval('public.generated_annual_reports_id_seq'::regclass);


--
-- Name: identities id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identities ALTER COLUMN id SET DEFAULT nextval('public.identities_id_seq'::regclass);


--
-- Name: instance_moderation_notes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.instance_moderation_notes ALTER COLUMN id SET DEFAULT nextval('public.instance_moderation_notes_id_seq'::regclass);


--
-- Name: invites id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites ALTER COLUMN id SET DEFAULT nextval('public.invites_id_seq'::regclass);


--
-- Name: ip_blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ip_blocks ALTER COLUMN id SET DEFAULT nextval('public.ip_blocks_id_seq'::regclass);


--
-- Name: keypairs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.keypairs ALTER COLUMN id SET DEFAULT nextval('public.keypairs_id_seq'::regclass);


--
-- Name: list_accounts id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.list_accounts ALTER COLUMN id SET DEFAULT nextval('public.list_accounts_id_seq'::regclass);


--
-- Name: lists id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.lists ALTER COLUMN id SET DEFAULT nextval('public.lists_id_seq'::regclass);


--
-- Name: login_activities id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.login_activities ALTER COLUMN id SET DEFAULT nextval('public.login_activities_id_seq'::regclass);


--
-- Name: markers id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.markers ALTER COLUMN id SET DEFAULT nextval('public.markers_id_seq'::regclass);


--
-- Name: mentions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mentions ALTER COLUMN id SET DEFAULT nextval('public.mentions_id_seq'::regclass);


--
-- Name: mutes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mutes ALTER COLUMN id SET DEFAULT nextval('public.mutes_id_seq'::regclass);


--
-- Name: notification_permissions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_permissions ALTER COLUMN id SET DEFAULT nextval('public.notification_permissions_id_seq'::regclass);


--
-- Name: notification_policies id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_policies ALTER COLUMN id SET DEFAULT nextval('public.notification_policies_id_seq'::regclass);


--
-- Name: notifications id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications ALTER COLUMN id SET DEFAULT nextval('public.notifications_id_seq'::regclass);


--
-- Name: oauth_access_grants id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_grants ALTER COLUMN id SET DEFAULT nextval('public.oauth_access_grants_id_seq'::regclass);


--
-- Name: oauth_access_tokens id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_tokens ALTER COLUMN id SET DEFAULT nextval('public.oauth_access_tokens_id_seq'::regclass);


--
-- Name: oauth_applications id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_applications ALTER COLUMN id SET DEFAULT nextval('public.oauth_applications_id_seq'::regclass);


--
-- Name: pghero_space_stats id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pghero_space_stats ALTER COLUMN id SET DEFAULT nextval('public.pghero_space_stats_id_seq'::regclass);


--
-- Name: poll_votes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes ALTER COLUMN id SET DEFAULT nextval('public.poll_votes_id_seq'::regclass);


--
-- Name: polls id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.polls ALTER COLUMN id SET DEFAULT nextval('public.polls_id_seq'::regclass);


--
-- Name: preview_card_providers id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_card_providers ALTER COLUMN id SET DEFAULT nextval('public.preview_card_providers_id_seq'::regclass);


--
-- Name: preview_card_trends id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_card_trends ALTER COLUMN id SET DEFAULT nextval('public.preview_card_trends_id_seq'::regclass);


--
-- Name: preview_cards id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_cards ALTER COLUMN id SET DEFAULT nextval('public.preview_cards_id_seq'::regclass);


--
-- Name: relationship_severance_events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.relationship_severance_events ALTER COLUMN id SET DEFAULT nextval('public.relationship_severance_events_id_seq'::regclass);


--
-- Name: relays id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.relays ALTER COLUMN id SET DEFAULT nextval('public.relays_id_seq'::regclass);


--
-- Name: report_notes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_notes ALTER COLUMN id SET DEFAULT nextval('public.report_notes_id_seq'::regclass);


--
-- Name: reports id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports ALTER COLUMN id SET DEFAULT nextval('public.reports_id_seq'::regclass);


--
-- Name: rule_translations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rule_translations ALTER COLUMN id SET DEFAULT nextval('public.rule_translations_id_seq'::regclass);


--
-- Name: rules id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rules ALTER COLUMN id SET DEFAULT nextval('public.rules_id_seq'::regclass);


--
-- Name: scheduled_statuses id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scheduled_statuses ALTER COLUMN id SET DEFAULT nextval('public.scheduled_statuses_id_seq'::regclass);


--
-- Name: session_activations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.session_activations ALTER COLUMN id SET DEFAULT nextval('public.session_activations_id_seq'::regclass);


--
-- Name: settings id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings ALTER COLUMN id SET DEFAULT nextval('public.settings_id_seq'::regclass);


--
-- Name: severed_relationships id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.severed_relationships ALTER COLUMN id SET DEFAULT nextval('public.severed_relationships_id_seq'::regclass);


--
-- Name: site_uploads id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.site_uploads ALTER COLUMN id SET DEFAULT nextval('public.site_uploads_id_seq'::regclass);


--
-- Name: software_updates id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.software_updates ALTER COLUMN id SET DEFAULT nextval('public.software_updates_id_seq'::regclass);


--
-- Name: status_edits id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_edits ALTER COLUMN id SET DEFAULT nextval('public.status_edits_id_seq'::regclass);


--
-- Name: status_pins id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_pins ALTER COLUMN id SET DEFAULT nextval('public.status_pins_id_seq'::regclass);


--
-- Name: status_stats id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_stats ALTER COLUMN id SET DEFAULT nextval('public.status_stats_id_seq'::regclass);


--
-- Name: status_trends id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_trends ALTER COLUMN id SET DEFAULT nextval('public.status_trends_id_seq'::regclass);


--
-- Name: tag_follows id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_follows ALTER COLUMN id SET DEFAULT nextval('public.tag_follows_id_seq'::regclass);


--
-- Name: tag_trends id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_trends ALTER COLUMN id SET DEFAULT nextval('public.tag_trends_id_seq'::regclass);


--
-- Name: tagged_objects id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tagged_objects ALTER COLUMN id SET DEFAULT nextval('public.tagged_objects_id_seq'::regclass);


--
-- Name: tags id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags ALTER COLUMN id SET DEFAULT nextval('public.tags_id_seq'::regclass);


--
-- Name: terms_of_services id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.terms_of_services ALTER COLUMN id SET DEFAULT nextval('public.terms_of_services_id_seq'::regclass);


--
-- Name: tombstones id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tombstones ALTER COLUMN id SET DEFAULT nextval('public.tombstones_id_seq'::regclass);


--
-- Name: unavailable_domains id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unavailable_domains ALTER COLUMN id SET DEFAULT nextval('public.unavailable_domains_id_seq'::regclass);


--
-- Name: user_invite_requests id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_invite_requests ALTER COLUMN id SET DEFAULT nextval('public.user_invite_requests_id_seq'::regclass);


--
-- Name: user_roles id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_roles ALTER COLUMN id SET DEFAULT nextval('public.user_roles_id_seq'::regclass);


--
-- Name: username_blocks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.username_blocks ALTER COLUMN id SET DEFAULT nextval('public.username_blocks_id_seq'::regclass);


--
-- Name: users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);


--
-- Name: web_push_subscriptions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_push_subscriptions ALTER COLUMN id SET DEFAULT nextval('public.web_push_subscriptions_id_seq'::regclass);


--
-- Name: web_settings id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_settings ALTER COLUMN id SET DEFAULT nextval('public.web_settings_id_seq'::regclass);


--
-- Name: webauthn_credentials id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials ALTER COLUMN id SET DEFAULT nextval('public.webauthn_credentials_id_seq'::regclass);


--
-- Name: webhooks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhooks ALTER COLUMN id SET DEFAULT nextval('public.webhooks_id_seq'::regclass);


--
-- Data for Name: account_aliases; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_aliases (id, account_id, acct, created_at, updated_at, uri) FROM stdin;
\.


--
-- Data for Name: account_conversations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_conversations (id, account_id, conversation_id, last_status_id, lock_version, participant_account_ids, status_ids, unread) FROM stdin;
9302	116844606259201001	9301	116844853985285004	0	{116844606259202001}	{116844853985285004,116846257766400501}	t
\.


--
-- Data for Name: account_deletion_requests; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_deletion_requests (id, account_id, created_at, updated_at) FROM stdin;
\.


--
-- Data for Name: account_domain_blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_domain_blocks (id, account_id, created_at, domain, updated_at) FROM stdin;
9504	116844606259201001	2026-07-01 15:07:00	account-blocked.fixture.invalid	2026-07-01 15:07:00
\.


--
-- Data for Name: account_migrations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_migrations (id, account_id, acct, created_at, followers_count, target_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: account_moderation_notes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_moderation_notes (id, account_id, content, created_at, target_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: account_notes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_notes (id, account_id, comment, created_at, target_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: account_pins; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_pins (id, account_id, created_at, target_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: account_relationship_severance_events; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_relationship_severance_events (id, account_id, created_at, followers_count, following_count, relationship_severance_event_id, updated_at) FROM stdin;
8302	116844606259201001	2026-07-01 17:00:00	0	1	8301	2026-07-01 17:00:00
\.


--
-- Data for Name: account_stats; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_stats (id, account_id, created_at, followers_count, following_count, last_status_at, statuses_count, updated_at) FROM stdin;
11001	116844606259201001	2026-07-01 18:10:00	1	1	2026-07-01 00:00:00	5	2026-07-01 18:10:00
11002	116844606259201002	2026-07-01 18:10:00	0	0	\N	0	2026-07-01 18:10:00
11003	116844606259201003	2026-07-01 18:10:00	0	0	\N	0	2026-07-01 18:10:00
11004	116844606259202001	2026-07-01 18:10:00	1	1	2026-07-01 00:00:00	7	2026-07-01 18:10:00
11005	116844606259202002	2026-07-01 18:10:00	0	0	\N	0	2026-07-01 18:10:00
11006	-99	2026-07-01 18:10:00	0	0	\N	0	2026-07-01 18:10:00
11007	116844606259202003	2026-07-01 18:10:00	0	0	\N	0	2026-07-01 18:10:00
\.


--
-- Data for Name: account_statuses_cleanup_policies; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_statuses_cleanup_policies (id, account_id, created_at, enabled, keep_direct, keep_media, keep_pinned, keep_polls, keep_self_bookmark, keep_self_fav, min_favs, min_reblogs, min_status_age, updated_at) FROM stdin;
\.


--
-- Data for Name: account_warning_presets; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_warning_presets (id, created_at, text, title, updated_at) FROM stdin;
\.


--
-- Data for Name: account_warnings; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.account_warnings (id, account_id, action, created_at, overruled_at, report_id, status_ids, target_account_id, text, updated_at) FROM stdin;
8401	116844606259201002	0	2026-07-01 17:20:00	\N	\N	{116844842188805001}	116844606259201001	Readable fixture moderation warning	2026-07-01 17:20:00
\.


--
-- Data for Name: accounts; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.accounts (id, actor_type, also_known_as, attribution_domains, avatar_content_type, avatar_description, avatar_file_name, avatar_file_size, avatar_remote_url, avatar_storage_schema_version, avatar_updated_at, collections_url, created_at, discoverable, display_name, domain, feature_approval_policy, featured_collection_url, fields, followers_url, following_url, header_content_type, header_description, header_file_name, header_file_size, header_remote_url, header_storage_schema_version, header_updated_at, hide_collections, id_scheme, inbox_url, indexable, last_webfingered_at, locked, memorial, moved_to_account_id, note, outbox_url, private_key, protocol, public_key, requested_review_at, reviewed_at, sensitized_at, shared_inbox_url, show_featured, show_media, show_media_replies, silenced_at, suspended_at, suspension_origin, trendable, updated_at, uri, url, username) FROM stdin;
116844606259201002	Person	\N	{}	\N		\N	\N	\N	\N	\N	\N	2026-07-01 12:00:00	f	Moderator Fixture	\N	0	\N	\N			\N		\N	\N		\N	\N	\N	1		f	\N	f	f	\N	Local moderator fixture		-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI\neHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn\nFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F\njr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn\nqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar\n+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3\nfkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd\nRRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC\nI1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh\nFBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk\nQcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu\nulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC\nSTD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO\nL2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6\nBTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7\ngWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X\n8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3\nqOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE\ncSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo\nzrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3\nlXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F\nrIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza\nGvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE\n+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO\n4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb\n-----END RSA PRIVATE KEY-----\n	0	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N		t	t	t	\N	\N	\N	\N	2026-07-01 12:00:00		https://fixture-v4-6-5.rustodon.invalid/@moderator	moderator
116844606259201003	Person	\N	{}	\N		\N	\N	\N	\N	\N	\N	2026-07-01 12:00:00	t	New User Fixture	\N	0	\N	\N			\N		\N	\N		\N	\N	\N	1		t	\N	f	f	\N	Local sign-up notification actor		-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI\neHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn\nFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F\njr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn\nqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar\n+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3\nfkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd\nRRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC\nI1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh\nFBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk\nQcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu\nulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC\nSTD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO\nL2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6\nBTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7\ngWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X\n8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3\nqOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE\ncSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo\nzrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3\nlXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F\nrIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza\nGvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE\n+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO\n4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb\n-----END RSA PRIVATE KEY-----\n	0	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N		t	t	t	\N	\N	\N	\N	2026-07-01 12:00:00		https://fixture-v4-6-5.rustodon.invalid/@newbie	newbie
116844606259202002	Person	\N	{}	\N		\N	\N	\N	\N	\N	\N	2026-07-01 12:00:00	t	Carol Remote	remote.fixture.invalid	0	\N	\N	https://remote.fixture.invalid/users/carol/followers	https://remote.fixture.invalid/users/carol/following	\N		\N	\N		\N	\N	\N	1	https://remote.fixture.invalid/users/carol/inbox	t	\N	f	f	\N	Remote pending follower	https://remote.fixture.invalid/users/carol/outbox	\N	0	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N	https://remote.fixture.invalid/inbox	t	t	t	\N	\N	\N	\N	2026-07-01 12:00:00	https://remote.fixture.invalid/users/carol	https://remote.fixture.invalid/@carol	carol
116844606259202001	Person	{https://alias.remote.fixture.invalid/users/bob}	{media.remote.fixture.invalid}	\N		\N	\N	\N	\N	\N	https://remote.fixture.invalid/users/bob/collections	2026-07-01 12:00:00	t	Bob Remote	remote.fixture.invalid	0	https://remote.fixture.invalid/users/bob/collections/featured	[{"name": "Fixture field", "value": "Exact JSONB value", "verified_at": null}]	https://remote.fixture.invalid/users/bob/followers	https://remote.fixture.invalid/users/bob/following	\N		\N	\N		\N	\N	\N	1	https://remote.fixture.invalid/users/bob/inbox	t	\N	f	f	\N	Primary remote fixture actor	https://remote.fixture.invalid/users/bob/outbox	\N	0	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N	https://remote.fixture.invalid/inbox	t	t	t	\N	\N	\N	\N	2026-07-01 12:00:00	https://remote.fixture.invalid/users/bob	https://remote.fixture.invalid/@bob	bob
116844606259201001	Person	\N	{}	image/png	Deterministic Mastodon test avatar	0112603425bb49c1.png	329555	\N	1	2026-07-01 12:00:00	\N	2026-07-01 12:00:00	t	Alice Fixture	\N	0	\N	\N			\N		\N	\N		\N	\N	\N	0		t	\N	t	f	\N	Primary local fixture account		-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI\neHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn\nFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F\njr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn\nqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar\n+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3\nfkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd\nRRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC\nI1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh\nFBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk\nQcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu\nulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC\nSTD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO\nL2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6\nBTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7\ngWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X\n8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3\nqOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE\ncSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo\nzrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3\nlXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F\nrIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza\nGvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE\n+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO\n4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb\n-----END RSA PRIVATE KEY-----\n	0	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N		t	t	t	\N	\N	\N	\N	2026-07-01 12:00:00		https://fixture-v4-6-5.rustodon.invalid/@alice	alice
-99	Application	\N	\N	\N		\N	\N	\N	\N	\N	\N	2026-07-01 12:00:00	f	Fixture Instance Actor	\N	0	\N	\N	https://fixture-v4-6-5.rustodon.invalid/actor/followers	https://fixture-v4-6-5.rustodon.invalid/actor/following	\N		\N	\N		\N	\N	\N	1	https://fixture-v4-6-5.rustodon.invalid/actor/inbox	f	\N	f	f	\N	Local service actor without a login-capable user	https://fixture-v4-6-5.rustodon.invalid/actor/outbox	-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAqIAYvNFGbZ5g4iiK6feSdXD4bDStFM58A7tHycYXaYtzZQpI\neHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S07t0V9wNK94he01LV5EMz/GN4eNn\nFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZIFLSb96Q5w0Z/k7ntpVKV52y8kz5F\njr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSnaKvT7P9jhgC6uTre+jXyvVZjiHDrn\nqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ+OuPRI1URIWQI01DCHqcohVu9+Ar\n+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+8QIDAQABAoIBAAgySHnFWI6gItR3\nfkfiqIm80cHCN3Xk1C6iiVu+3oBOZbHpW9R7vl9e/WOA/9O+LPjiSsQOegtWnVvd\nRRjrl7Hj20VDlZKv5Mssm6zOGAxksrcVbqwdj+fUJaNJCL0AyyseH0x/IE9T8rDC\nI1GH+3tB3JkhkIN/qjipdX5ab8MswEPu8IC4ViTpdBgWYY/xBcAHPw4xuL0tcwzh\nFBlf4DqoEVQo8GdK5GAJ2Ny0S4xbXHUURzx/R4y4CCts7niAiLGqd9jmLU1kUTMk\nQcXfQYK6l+unLc7wDYAz7sFEHh04M48VjWwiIZJnlCqmQbLda7uhhu8zkF1DqZTu\nulWDGQECgYEA0TIAc8BQBVab979DHEEmMdgqBwxLY3OIAk0b+r50h7VBGWCDPRsC\nSTD73fQY3lNet/7/jgSGwwAlAJ5PpMXxXiZAE3bUwPmHzgF7pvIOOLhA8O07tHSO\nL2mvQe6NPzjZ+6iAO2U9PkClxcvGvPx2OBvisfHqZLmxC9PIVxzruQECgYEAzjM6\nBTUXa6T/qHvLFbN699BXsUOGmHBGaLRapFDBfVvgZrwqYQcZpBBhesLdGTGSqwE7\ngWsITPIJ+Ldo+38oGYyVys+w/V67q6ud7hgSDTW3hSvm+GboCjk6gzxlt9hQ0t9X\n8vfDOYhEXvVUJNv3mYO60ENqQhILO4bQ0zi+VfECgYBb/nUccfG+pzunU0Cb6Dp3\nqOuydcGhVmj1OhuXxLFSDG84Tazo7juvHA9mp7VX76mzmDuhpHPuxN2AzB2SBEoE\ncSW0aYld413JRfWukLuYTc6hJHIhBTCRwRQFFnae2s1hUdQySm8INT2xIc+fxBXo\nzrp+Ljg5Wz90SAnN5TX0AQKBgDaatDOq0o/r+tPYLHiLtfWoE4Dau+rkWJDjqdk3\nlXWn/e3WyHY3Vh/vQpEqxzgju45TXjmwaVtPATr+/usSykCxzP0PMPR3wMT+Rm1F\nrIoY/odij+CaB7qlWwxj0x/zRbwB7x1lZSp4HnrzBpxYL+JUUwVRxPLIKndSBTza\nGvVRAoGBAIVBcNcRQYF4fvZjDKAb4fdBsEuHmycqtRCsnkGOz6ebbEQznSaZ0tZE\n+JuouZaGjyp8uPjNGD5D7mIGbyoZ3KyG4mTXNxDAGBso1hrNDKGBOrGaPhZx8LgO\n4VXJ+ybXrATf4jr8ccZYsZdFpOphPzz+j55Mqg5vac5P1XjmsGTb\n-----END RSA PRIVATE KEY-----\n	0	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N	https://fixture-v4-6-5.rustodon.invalid/inbox	t	t	t	\N	\N	\N	\N	2026-07-01 12:00:00	https://fixture-v4-6-5.rustodon.invalid/actor	https://fixture-v4-6-5.rustodon.invalid/actor	fixture-v4-6-5.rustodon.invalid
116844606259202003	Person	\N	{}	\N		\N	\N	\N	\N	\N	\N	2026-07-01 12:00:00	\N	Suspended Remote Fixture	remote.fixture.invalid	0	\N	\N	https://remote.fixture.invalid/users/suspended/followers	https://remote.fixture.invalid/users/suspended/following	\N		\N	\N		\N	\N	\N	1	https://remote.fixture.invalid/users/suspended/inbox	f	\N	f	f	\N	Suspended sender exclusion coverage	https://remote.fixture.invalid/users/suspended/outbox	\N	1	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	\N	\N	\N	https://remote.fixture.invalid/inbox	t	t	t	\N	2026-07-01 18:30:00	\N	\N	2026-07-01 18:30:00	https://remote.fixture.invalid/users/suspended	https://remote.fixture.invalid/@suspended	suspended
\.


--
-- Data for Name: accounts_tags; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.accounts_tags (account_id, tag_id) FROM stdin;
116844606259201001	9201
\.


--
-- Data for Name: admin_action_logs; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.admin_action_logs (id, account_id, action, created_at, human_identifier, permalink, route_param, target_id, target_type, updated_at) FROM stdin;
\.


--
-- Data for Name: announcement_mutes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.announcement_mutes (id, account_id, announcement_id, created_at, updated_at) FROM stdin;
\.


--
-- Data for Name: announcement_reactions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.announcement_reactions (id, account_id, announcement_id, created_at, custom_emoji_id, name, updated_at) FROM stdin;
\.


--
-- Data for Name: announcements; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.announcements (id, all_day, created_at, ends_at, notification_sent_at, published, published_at, scheduled_at, starts_at, status_ids, text, updated_at) FROM stdin;
\.


--
-- Data for Name: annual_report_statuses_per_account_counts; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.annual_report_statuses_per_account_counts (id, account_id, statuses_count, year) FROM stdin;
\.


--
-- Data for Name: appeals; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.appeals (id, account_id, account_warning_id, approved_at, approved_by_account_id, created_at, rejected_at, rejected_by_account_id, text, updated_at) FROM stdin;
\.


--
-- Data for Name: ar_internal_metadata; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.ar_internal_metadata (key, value, created_at, updated_at) FROM stdin;
environment	production	2026-07-01 00:00:00	2026-07-01 00:00:00
schema_sha1	c7b4869d1d9d5614d86e2723464402c4976f6075	2026-07-01 00:00:00	2026-07-01 00:00:00
\.


--
-- Data for Name: backups; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.backups (id, created_at, dump_content_type, dump_file_name, dump_file_size, dump_updated_at, processed, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.blocks (id, account_id, created_at, target_account_id, updated_at, uri) FROM stdin;
9502	116844606259201001	2026-07-01 15:05:00	116844606259202002	2026-07-01 15:05:00	https://fixture-v4-6-5.rustodon.invalid/users/alice#blocks/9502
\.


--
-- Data for Name: bookmarks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.bookmarks (id, account_id, created_at, status_id, updated_at) FROM stdin;
9501	116844606259201001	2026-07-01 15:04:00	116845078118405101	2026-07-01 15:04:00
9507	116844606259201001	2026-07-01 19:10:00	116846257766400501	2026-07-01 19:10:00
\.


--
-- Data for Name: bulk_import_rows; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.bulk_import_rows (id, bulk_import_id, created_at, data, updated_at) FROM stdin;
\.


--
-- Data for Name: bulk_imports; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.bulk_imports (id, account_id, created_at, finished_at, imported_items, likely_mismatched, missing_status, original_filename, overwrite, processed_items, state, total_items, type, updated_at) FROM stdin;
\.


--
-- Data for Name: canonical_email_blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.canonical_email_blocks (id, canonical_email_hash, created_at, reference_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: collection_items; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.collection_items (id, account_id, activity_uri, approval_last_verified_at, approval_uri, collection_id, created_at, object_uri, "position", state, updated_at, uri) FROM stdin;
116845549977608802	116844606259201001	https://remote.fixture.invalid/activities/add-116845549977608802	2026-07-01 16:01:00	\N	116845549977608801	2026-07-01 16:00:00	https://fixture-v4-6-5.rustodon.invalid/users/alice	1	1	2026-07-01 16:01:00	\N
\.


--
-- Data for Name: collection_reports; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.collection_reports (id, collection_id, created_at, report_id, updated_at) FROM stdin;
\.


--
-- Data for Name: collections; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.collections (id, account_id, created_at, description, description_html, discoverable, item_count, language, local, name, original_number_of_items, sensitive, tag_id, updated_at, uri, url) FROM stdin;
116845549977608801	116844606259202001	2026-07-01 16:00:00	Bob features Alice	<p>Bob features Alice</p>	t	1	en	f	Remote fixture collection	1	f	\N	2026-07-01 16:00:00	https://remote.fixture.invalid/users/bob/collections/116845549977608801	https://remote.fixture.invalid/@bob/collections/116845549977608801
\.


--
-- Data for Name: conversation_mutes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.conversation_mutes (id, account_id, conversation_id) FROM stdin;
9303	116844606259201001	9301
\.


--
-- Data for Name: conversations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.conversations (id, created_at, parent_account_id, parent_status_id, updated_at, uri) FROM stdin;
9301	2026-07-01 13:03:00	116844606259201001	116844853985285004	2026-07-01 13:03:00	https://fixture-v4-6-5.rustodon.invalid/conversations/9301
\.


--
-- Data for Name: custom_emoji_categories; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.custom_emoji_categories (id, created_at, featured_emoji_id, name, updated_at) FROM stdin;
\.


--
-- Data for Name: custom_emojis; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.custom_emojis (id, category_id, created_at, disabled, domain, image_content_type, image_file_name, image_file_size, image_remote_url, image_storage_schema_version, image_updated_at, shortcode, updated_at, uri, visible_in_picker) FROM stdin;
\.


--
-- Data for Name: custom_filter_keywords; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.custom_filter_keywords (id, created_at, custom_filter_id, keyword, updated_at, whole_word) FROM stdin;
9102	2026-07-01 12:22:00	9101	spoiler fixture	2026-07-01 12:22:00	t
\.


--
-- Data for Name: custom_filter_statuses; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.custom_filter_statuses (id, created_at, custom_filter_id, status_id, updated_at) FROM stdin;
9103	2026-07-01 12:22:00	9101	116845105643525105	2026-07-01 12:22:00
9104	2026-07-01 19:10:00	9101	116846257766400501	2026-07-01 19:10:00
\.


--
-- Data for Name: custom_filters; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.custom_filters (id, account_id, action, context, created_at, expires_at, phrase, updated_at) FROM stdin;
9101	116844606259201001	1	{home,notifications}	2026-07-01 12:22:00	\N	Readable fixture filter	2026-07-01 12:22:00
\.


--
-- Data for Name: domain_allows; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.domain_allows (id, created_at, domain, updated_at) FROM stdin;
9601	2026-07-01 15:10:00	allowed.fixture.invalid	2026-07-01 15:10:00
\.


--
-- Data for Name: domain_blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.domain_blocks (id, created_at, domain, obfuscate, private_comment, public_comment, reject_media, reject_reports, severity, updated_at) FROM stdin;
9602	2026-07-01 15:11:00	blocked.fixture.invalid	t	Raw private moderation note		t	f	99	2026-07-01 15:11:00
\.


--
-- Data for Name: email_domain_blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.email_domain_blocks (id, allow_with_approval, created_at, domain, parent_id, updated_at) FROM stdin;
\.


--
-- Data for Name: email_subscriptions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.email_subscriptions (id, account_id, confirmation_token, confirmed_at, created_at, email, locale, updated_at) FROM stdin;
\.


--
-- Data for Name: fasp_backfill_requests; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.fasp_backfill_requests (id, category, created_at, cursor, fasp_provider_id, fulfilled, max_count, updated_at) FROM stdin;
\.


--
-- Data for Name: fasp_debug_callbacks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.fasp_debug_callbacks (id, created_at, fasp_provider_id, ip, request_body, updated_at) FROM stdin;
\.


--
-- Data for Name: fasp_follow_recommendations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.fasp_follow_recommendations (id, created_at, recommended_account_id, requesting_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: fasp_providers; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.fasp_providers (id, base_url, capabilities, confirmed, contact_email, created_at, delivery_last_failed_at, fediverse_account, name, privacy_policy, provider_public_key_pem, remote_identifier, server_private_key_pem, sign_in_url, updated_at) FROM stdin;
\.


--
-- Data for Name: fasp_subscriptions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.fasp_subscriptions (id, category, created_at, fasp_provider_id, max_batch_size, subscription_type, threshold_likes, threshold_replies, threshold_shares, threshold_timeframe, updated_at) FROM stdin;
\.


--
-- Data for Name: favourites; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.favourites (id, account_id, created_at, status_id, updated_at) FROM stdin;
8101	116844606259202001	2026-07-01 15:03:00	116844842188805001	2026-07-01 15:03:00
8102	116844606259202001	2026-07-01 19:10:00	116846257766400501	2026-07-01 19:10:00
\.


--
-- Data for Name: featured_tags; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.featured_tags (id, account_id, created_at, last_status_at, name, statuses_count, tag_id, updated_at) FROM stdin;
9202	116844606259201001	2026-07-01 13:00:00	2026-07-01 13:00:00	fixturetag	1	9201	2026-07-01 13:00:00
\.


--
-- Data for Name: follow_recommendation_mutes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.follow_recommendation_mutes (id, account_id, created_at, target_account_id, updated_at) FROM stdin;
\.


--
-- Data for Name: follow_recommendation_suppressions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.follow_recommendation_suppressions (id, account_id, created_at, updated_at) FROM stdin;
\.


--
-- Data for Name: follow_requests; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.follow_requests (id, account_id, created_at, languages, notify, show_reblogs, target_account_id, updated_at, uri) FROM stdin;
8003	116844606259202002	2026-07-01 12:12:00	{en}	f	t	116844606259201001	2026-07-01 12:12:00	https://remote.fixture.invalid/users/carol#follows/8003
\.


--
-- Data for Name: follows; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.follows (id, account_id, created_at, languages, notify, show_reblogs, target_account_id, updated_at, uri) FROM stdin;
8001	116844606259201001	2026-07-01 12:10:00	{en}	t	t	116844606259202001	2026-07-01 12:10:00	https://fixture-v4-6-5.rustodon.invalid/users/alice#follows/8001
8002	116844606259202001	2026-07-01 12:11:00	\N	f	t	116844606259201001	2026-07-01 12:11:00	https://remote.fixture.invalid/users/bob#follows/8002
\.


--
-- Data for Name: generated_annual_reports; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.generated_annual_reports (id, account_id, created_at, data, schema_version, share_key, updated_at, viewed_at, year) FROM stdin;
8501	116844606259201001	2026-07-01 17:30:00	{"top_statuses": {"most_reblogged": 116844842188805001}, "most_reblogged_accounts": [], "commonly_interacted_with_accounts": []}	2	fixture-annual-report-share-key	2026-07-01 17:30:00	\N	2025
\.


--
-- Data for Name: identities; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.identities (id, created_at, provider, uid, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: instance_moderation_notes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.instance_moderation_notes (id, account_id, content, created_at, domain, updated_at) FROM stdin;
\.


--
-- Data for Name: invites; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.invites (id, autofollow, code, comment, created_at, expires_at, max_uses, updated_at, user_id, uses) FROM stdin;
\.


--
-- Data for Name: ip_blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.ip_blocks (id, comment, created_at, expires_at, ip, severity, updated_at) FROM stdin;
\.


--
-- Data for Name: keypairs; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.keypairs (id, account_id, created_at, expires_at, private_key, public_key, revoked, type, updated_at, uri) FROM stdin;
8901	116844606259202001	2026-07-01 16:10:00	\N	\N	-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqIAYvNFGbZ5g4iiK6feS\ndXD4bDStFM58A7tHycYXaYtzZQpIeHXAmaXuZzXIwtrP4N0gIk8JNwZvXj2UPS+S\n07t0V9wNK94he01LV5EMz/GN4eNnFmDL64HIEuKLvV8TvgjbUPRD6Y5X0UpKi2ZI\nFLSb96Q5w0Z/k7ntpVKV52y8kz5Fjr/O/0JuHryZe0yItzJh8kzFfeMf0EXzfSna\nKvT7P9jhgC6uTre+jXyvVZjiHDrnqvvucdI3I7DRfXo1OqARBrLjy+TdseUAjNYJ\n+OuPRI1URIWQI01DCHqcohVu9+Ar+BiCjFp3ua+XMuJvrvbD61d1Fvig/9nbBRR+\n8QIDAQAB\n-----END PUBLIC KEY-----\n	f	0	2026-07-01 16:10:00	https://remote.fixture.invalid/users/bob#secondary-key
8902	116844606259201001	2026-07-01 16:11:00	\N	{"p":"9q4gHslWnbK8a5zNjL1ySdpX5I8wunpM6ed7whTvAnwlyUs=","h":{"iv":"jvVW4mOA+IzCBTv/","at":"Wd+qB8DnQnN2bRo/7ukJmQ=="}}	opaque-public-key-material	t	0	2026-07-01 16:11:00	https://fixture-v4-6-5.rustodon.invalid/users/alice#opaque-key
\.


--
-- Data for Name: list_accounts; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.list_accounts (id, account_id, follow_id, follow_request_id, list_id) FROM stdin;
9003	116844606259202001	8001	\N	9001
9004	116844606259202001	8001	\N	9002
\.


--
-- Data for Name: lists; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.lists (id, account_id, created_at, exclusive, replies_policy, title, updated_at) FROM stdin;
9001	116844606259201001	2026-07-01 12:20:00	f	0	Fixture normal list	2026-07-01 12:20:00
9002	116844606259201001	2026-07-01 12:21:00	t	1	Fixture exclusive list	2026-07-01 12:21:00
\.


--
-- Data for Name: login_activities; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.login_activities (id, authentication_method, created_at, failure_reason, ip, provider, success, user_agent, user_id) FROM stdin;
\.


--
-- Data for Name: markers; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.markers (id, created_at, last_read_id, lock_version, timeline, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: media_attachments; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.media_attachments (id, account_id, blurhash, created_at, description, file_content_type, file_file_name, file_file_size, file_meta, file_storage_schema_version, file_updated_at, processing, remote_url, scheduled_status_id, shortcode, status_id, thumbnail_content_type, thumbnail_file_name, thumbnail_file_size, thumbnail_remote_url, thumbnail_storage_schema_version, thumbnail_updated_at, type, updated_at) FROM stdin;
-101	116844606259201001	\N	2026-07-01 13:00:00	First ordered attachment	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/first.jpg	\N	\N	116844842188805001	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:00:00
-102	116844606259201001	\N	2026-07-01 13:00:00	Third ordered attachment	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/third.jpg	\N	\N	116844842188805001	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:00:00
-103	116844606259201001	\N	2026-07-01 13:00:00	Fourth ordered attachment	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fourth.jpg	\N	\N	116844842188805001	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:00:00
-104	116844606259201001	\N	2026-07-01 13:00:00	Over-limit ordered attachment	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fifth.jpg	\N	\N	116844842188805001	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:00:00
-105	116844606259201001	\N	2026-07-01 13:00:00	Stale unordered attachment	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/stale.jpg	\N	\N	116844842188805001	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:00:00
-210	116844606259201001	\N	2026-07-01 13:01:00	NULL-order fallback first	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fallback-first.jpg	\N	\N	116844846120965002	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:01:00
-209	116844606259201001	\N	2026-07-01 13:01:00	NULL-order fallback second	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fallback-second.jpg	\N	\N	116844846120965002	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:01:00
-208	116844606259201001	\N	2026-07-01 13:01:00	NULL-order fallback third	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fallback-third.jpg	\N	\N	116844846120965002	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:01:00
-207	116844606259201001	\N	2026-07-01 13:01:00	NULL-order fallback fourth	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fallback-fourth.jpg	\N	\N	116844846120965002	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:01:00
-206	116844606259201001	\N	2026-07-01 13:01:00	NULL-order fallback over limit	\N	\N	\N	\N	\N	\N	2	https://media.fixture.invalid/fallback-fifth.jpg	\N	\N	116844846120965002	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:01:00
-98	116844606259201001	\N	2026-07-01 19:10:00	Deleted status media must not be returned	\N	\N	\N	\N	\N	\N	2		\N	\N	116846257766400501	\N	\N	\N	\N	\N	\N	0	2026-07-01 19:10:00
116844842188806001	116844606259201001	UDKw:zyZ.9xs?KKQocn#0;-;%1i^Rk-.IVIU	2026-07-01 13:00:00	Deterministic Mastodon test attachment	image/jpeg	cd63911ad76f4d5d.jpg	36381	{"original":{"width":600,"height":400,"size":"600x400","aspect":1.5},"small":{"width":588,"height":392,"size":"588x392","aspect":1.5}}	1	2026-07-01 13:00:00	2		\N	\N	116844842188805001	\N	\N	\N	\N	\N	\N	0	2026-07-01 13:00:00
\.


--
-- Data for Name: mentions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.mentions (id, account_id, created_at, silent, status_id, updated_at) FROM stdin;
7001	116844606259201001	2026-07-01 14:00:00	f	116845078118405101	2026-07-01 14:00:00
7002	116844606259202001	2026-07-01 13:03:00	f	116844853985285004	2026-07-01 13:03:00
7003	116844606259202001	2026-07-01 19:10:00	f	116846257766400501	2026-07-01 19:10:00
\.


--
-- Data for Name: mutes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.mutes (id, account_id, created_at, expires_at, hide_notifications, target_account_id, updated_at) FROM stdin;
9503	116844606259201001	2026-07-01 15:06:00	2026-08-01 00:00:00	f	116844606259202001	2026-07-01 15:06:00
\.


--
-- Data for Name: notification_permissions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.notification_permissions (id, account_id, created_at, from_account_id, updated_at) FROM stdin;
9702	116844606259201001	2026-07-01 15:13:00	116844606259202001	2026-07-01 15:13:00
\.


--
-- Data for Name: notification_policies; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.notification_policies (id, account_id, created_at, for_bots, for_limited_accounts, for_new_accounts, for_not_followers, for_not_following, for_private_mentions, updated_at) FROM stdin;
9701	116844606259201001	2026-07-01 15:12:00	0	1	2	0	99	1	2026-07-01 15:12:00
\.


--
-- Data for Name: notification_requests; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.notification_requests (id, account_id, created_at, from_account_id, last_status_id, notifications_count, updated_at) FROM stdin;
116846261698560601	116844606259201001	2026-07-01 19:01:00	116844606259202001	116845078118405101	1	2026-07-01 19:01:00
-96	116844606259201001	2026-07-01 19:02:00	116844606259202002	116846257766400501	1	2026-07-01 19:02:00
-95	116844606259201001	2026-07-01 19:03:00	116844606259202003	116844842188805001	1	2026-07-01 19:03:00
\.


--
-- Data for Name: notifications; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.notifications (id, account_id, activity_id, activity_type, created_at, filtered, from_account_id, group_key, type, updated_at) FROM stdin;
10001	116844606259201001	7001	Mention	2026-07-01 18:00:01	f	116844606259202001	\N	mention	2026-07-01 18:00:01
10002	116844606259201001	116845101711365104	Status	2026-07-01 18:00:02	f	116844606259202001	\N	status	2026-07-01 18:00:02
10003	116844606259201001	116845321912325301	Status	2026-07-01 18:00:03	f	116844606259202001	reblog-116844842188805001-495255	reblog	2026-07-01 18:00:03
10004	116844606259201001	8002	Follow	2026-07-01 18:00:04	f	116844606259202001	follow-495252	follow	2026-07-01 18:00:04
10005	116844606259201001	8003	FollowRequest	2026-07-01 18:00:05	f	116844606259202002	\N	follow_request	2026-07-01 18:00:05
10006	116844606259201001	8101	Favourite	2026-07-01 18:00:06	f	116844606259202001	favourite-116844842188805001-495255	favourite	2026-07-01 18:00:06
10007	116844606259201001	8201	Poll	2026-07-01 18:00:07	f	116844606259202001	\N	poll	2026-07-01 18:00:07
10008	116844606259201001	116845105643525105	Status	2026-07-01 18:00:08	f	116844606259202001	\N	update	2026-07-01 18:00:08
10009	116844606259201001	8302	AccountRelationshipSeveranceEvent	2026-07-01 18:00:09	f	116844606259201001	\N	severed_relationships	2026-07-01 18:00:09
10010	116844606259201001	8401	AccountWarning	2026-07-01 18:00:10	f	116844606259201001	\N	moderation_warning	2026-07-01 18:00:10
10011	116844606259201001	8501	GeneratedAnnualReport	2026-07-01 18:00:11	f	116844606259201001	\N	annual_report	2026-07-01 18:00:11
10012	116844606259201002	116844606259201003	Account	2026-07-01 18:00:12	f	116844606259201003	admin.sign_up-495252	admin.sign_up	2026-07-01 18:00:12
10013	116844606259201002	8601	Report	2026-07-01 18:00:13	f	116844606259201001	\N	admin.report	2026-07-01 18:00:13
10014	116844606259201001	116845317980168701	Quote	2026-07-01 18:00:14	f	116844606259202001	\N	quote	2026-07-01 18:00:14
10015	116844606259201001	116845314048005201	Status	2026-07-01 18:00:15	f	116844606259202001	\N	quoted_update	2026-07-01 18:00:15
10016	116844606259201001	116845549977608802	CollectionItem	2026-07-01 18:00:16	f	116844606259202001	\N	added_to_collection	2026-07-01 18:00:16
10017	116844606259201001	116845549977608801	Collection	2026-07-01 18:00:17	f	116844606259202001	\N	collection_update	2026-07-01 18:00:17
10018	116844606259201001	116846257766400501	FutureActivity	2026-07-01 18:00:18	t	116844606259202001	\N	future_event	2026-07-01 18:00:18
10019	116844606259201001	116846257766400501	FutureActivity	2026-07-01 18:00:19	t	116844606259202001	\N	\N	2026-07-01 18:00:19
10020	116844606259201001	116846257766400501	Status	2026-07-01 18:00:20	t	116844606259202001	\N	future_deleted_status	2026-07-01 18:00:20
10021	116844606259201001	116844606259202003	FutureActivity	2026-07-01 18:00:21	t	116844606259202003	\N	future_suspended	2026-07-01 18:00:21
\.


--
-- Data for Name: oauth_access_grants; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.oauth_access_grants (id, application_id, code_challenge, code_challenge_method, created_at, expires_in, redirect_uri, resource_owner_id, revoked_at, scopes, token) FROM stdin;
\.


--
-- Data for Name: oauth_access_tokens; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.oauth_access_tokens (id, application_id, created_at, expires_in, last_used_at, last_used_ip, refresh_token, resource_owner_id, revoked_at, scopes, token) FROM stdin;
401	301	2026-07-01 12:00:00	\N	2026-07-01 12:30:00	192.0.2.10	fixture-refresh-token-v4-6-5	101	\N	read write follow push	fixture-bearer-token-v4-6-5
\.


--
-- Data for Name: oauth_applications; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.oauth_applications (id, confidential, created_at, name, owner_id, owner_type, redirect_uri, scopes, secret, superapp, uid, updated_at, website) FROM stdin;
301	t	2026-07-01 12:00:00	Rustodon fixture client	101	User	urn:ietf:wg:oauth:2.0:oob	read write follow push	fixture-only-client-secret-v4-6-5	f	rustodon-fixture-client-v4-6-5	2026-07-01 12:00:00	https://fixture-client.invalid
\.


--
-- Data for Name: pghero_space_stats; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.pghero_space_stats (id, captured_at, database, relation, schema, size) FROM stdin;
\.


--
-- Data for Name: poll_votes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.poll_votes (id, account_id, choice, created_at, poll_id, updated_at, uri) FROM stdin;
8202	116844606259201001	0	2024-01-01 13:00:00	8201	2024-01-01 13:00:00	https://fixture-v4-6-5.rustodon.invalid/users/alice#votes/8202
8204	116844606259201001	0	2026-07-01 19:10:00	8203	2026-07-01 19:10:00	\N
\.


--
-- Data for Name: polls; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.polls (id, account_id, cached_tallies, created_at, expires_at, hide_totals, last_fetched_at, lock_version, multiple, options, status_id, updated_at, voters_count, votes_count) FROM stdin;
8201	116844606259202001	{1,0}	2024-01-01 12:00:00	2024-01-02 12:00:00	f	2024-01-02 12:00:00	0	f	{Tea,Coffee}	111680579174405102	2024-01-02 12:00:00	1	1
8203	116844606259201001	{1}	2026-07-01 19:00:00	2026-07-01 20:00:00	f	\N	0	f	{Deleted}	116846257766400501	2026-07-01 19:30:00	1	1
\.


--
-- Data for Name: preview_card_providers; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.preview_card_providers (id, created_at, domain, icon_content_type, icon_file_name, icon_file_size, icon_updated_at, requested_review_at, reviewed_at, trendable, updated_at) FROM stdin;
\.


--
-- Data for Name: preview_card_trends; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.preview_card_trends (id, allowed, language, preview_card_id, rank, score) FROM stdin;
\.


--
-- Data for Name: preview_cards; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.preview_cards (id, author_account_id, author_name, author_url, blurhash, created_at, description, embed_url, height, html, image_content_type, image_description, image_file_name, image_file_size, image_storage_schema_version, image_updated_at, language, link_type, max_score, max_score_at, provider_name, provider_url, published_at, title, trendable, type, unverified_author_account_id, updated_at, url, width) FROM stdin;
\.


--
-- Data for Name: preview_cards_statuses; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.preview_cards_statuses (preview_card_id, status_id, url) FROM stdin;
\.


--
-- Data for Name: quotes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.quotes (id, account_id, activity_uri, approval_uri, created_at, legacy, quoted_account_id, quoted_status_id, state, status_id, updated_at) FROM stdin;
116845317980168701	116844606259202001	https://remote.fixture.invalid/activities/quote-116845317980168701	\N	2026-07-01 15:01:00	f	116844606259201001	116844842188805001	1	116845317980165202	2026-07-01 15:01:00
116845314048008702	116844606259201001	https://fixture-v4-6-5.rustodon.invalid/users/alice/quote_requests/116845314048008702	https://remote.fixture.invalid/activities/accept-116845314048008702	2026-07-01 15:00:00	f	116844606259202001	116845093847045103	1	116845314048005201	2026-07-01 15:00:00
-97	116844606259201001	\N	\N	2026-07-01 19:10:00	f	116844606259201001	116844842188805001	0	116846257766400501	2026-07-01 19:10:00
-94	116844606259201001	\N	\N	2026-07-01 19:20:00	f	116844606259201001	116846257766400501	4	116844846120965002	2026-07-01 19:20:00
\.


--
-- Data for Name: relationship_severance_events; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.relationship_severance_events (id, created_at, purged, target_name, type, updated_at) FROM stdin;
8301	2026-07-01 17:00:00	f	blocked.fixture.invalid	0	2026-07-01 17:00:00
\.


--
-- Data for Name: relays; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.relays (id, created_at, follow_activity_id, inbox_url, state, updated_at) FROM stdin;
\.


--
-- Data for Name: report_notes; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.report_notes (id, account_id, content, created_at, report_id, updated_at) FROM stdin;
\.


--
-- Data for Name: reports; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.reports (id, account_id, action_taken_at, action_taken_by_account_id, application_id, assigned_account_id, category, comment, created_at, forwarded, rule_ids, status_ids, target_account_id, updated_at, uri) FROM stdin;
8601	116844606259201001	\N	\N	\N	\N	1000	Readable fixture report	2026-07-01 17:10:00	f	\N	{116845105643525105}	116844606259202001	2026-07-01 17:10:00	https://fixture-v4-6-5.rustodon.invalid/users/alice/reports/8601
\.


--
-- Data for Name: rule_translations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.rule_translations (id, created_at, hint, language, rule_id, text, updated_at) FROM stdin;
\.


--
-- Data for Name: rules; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.rules (id, created_at, deleted_at, hint, priority, text, updated_at) FROM stdin;
\.


--
-- Data for Name: scheduled_statuses; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.scheduled_statuses (id, account_id, params, scheduled_at) FROM stdin;
\.


--
-- Data for Name: schema_migrations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.schema_migrations (version) FROM stdin;
20260611150940
20260505155103
20260425144553
20260423141611
20260420124030
20260415133505
20260410083500
20260326112324
20260325151755
20260323105645
20260319142348
20260318144837
20260311212130
20260311152331
20260310095021
20260303144409
20260217154542
20260212131934
20260212113020
20260211132603
20260209143308
20260209142402
20260127141820
20260127141459
20260119153538
20260115153219
20251217091936
20251209093813
20251202140424
20251201155054
20251201154910
20251119093332
20251118115657
20251117023614
20251023210145
20251007142305
20251007100813
20251007100627
20251002140103
20250924170259
20250912082651
20250911163952
20250909100506
20250902221600
20250828222741
20250820084312
20250819100545
20250805075010
20250717003848
20250627132728
20250605110215
20250520204643
20250520192024
20250428104538
20250428095029
20250425134308
20250422085303
20250422085027
20250422084214
20250422083912
20250411095859
20250411094808
20250410144908
20250328153843
20250313123400
20250305074104
20250224144617
20250221143646
20250129144813
20250129144440
20250108111200
20250103131909
20241216224825
20241216224813
20241216224530
20241216224520
20241216224514
20241216224507
20241216224237
20241216224229
20241216224218
20241216224211
20241216223859
20241216223852
20241216223452
20241216223446
20241216223433
20241216223425
20241213170053
20241213170043
20241213170036
20241213170027
20241213130230
20241212154346
20241212154231
20241212153254
20241212153202
20241212153054
20241212152910
20241212152734
20241212152618
20241212152158
20241210140838
20241206131513
20241205163118
20241205162640
20241205135925
20241205135901
20241205103523
20241123224956
20241123160722
20241111141355
20241104082851
20241022214312
20241014010506
20241007071624
20240918233930
20240916190140
20240909014637
20240808125420
20240808124339
20240808124338
20240808114841
20240724181224
20240720140205
20240713171909
20240713171841
20240712064044
20240607094856
20240607094603
20240607093954
20240607093446
20240603195202
20240522041528
20240513123807
20240513095755
20240510192043
20240322161611
20240322130318
20240322125607
20240321160706
20240320163441
20240320140159
20240312105620
20240312100644
20240310123453
20240307180905
20240304090449
20240227191620
20240222203722
20240222193403
20240221211359
20240221195828
20240221195424
20240217171534
20240111033014
20240109103012
20231222100226
20231212073317
20231211234923
20231210154528
20231018193659
20231018193355
20231018193209
20231018192110
20231006183200
20230907150100
20230904134623
20230822081029
20230818142253
20230818141056
20230814223300
20230811103651
20230803112520
20230803082451
20230725213448
20230724160715
20230702151753
20230702131023
20230630145300
20230605085711
20230605085710
20230531154811
20230531153942
20230524194155
20230524192812
20230524190515
20230330155710
20230330140036
20230330135507
20230215074423
20230215074327
20230129023109
20221206114142
20221104133904
20221101190723
20221025171544
20221021055441
20221012181003
20221006061337
20220829192658
20220829192633
20220827195229
20220824233535
20220824164532
20220824164433
20220808101323
20220729171123
20220714171049
20220710102457
20220704024901
20220617202502
20220613110903
20220613110834
20220613110802
20220613110711
20220613110628
20220611212541
20220611210335
20220606044941
20220527114923
20220429101850
20220429101025
20220428114902
20220428114454
20220428112727
20220428112511
20220316233212
20220310060959
20220310060939
20220310060926
20220310060913
20220310060854
20220310060833
20220310060809
20220310060750
20220310060740
20220310060722
20220310060706
20220310060653
20220310060641
20220310060626
20220310060614
20220310060556
20220310060545
20220309213005
20220307094650
20220307083603
20220304195405
20220303203437
20220303000827
20220302232632
20220227041951
20220224010024
20220210153119
20220202201015
20220202200926
20220202200743
20220124141035
20220118183123
20220118183010
20220116202951
20220115125341
20220115125126
20220109213908
20220105163928
20211231080958
20211213040746
20211126000907
20211123212714
20211115032527
20211112011713
20211031031021
20210908220918
20210904215403
20210808071221
20210722120340
20210630000137
20210621221010
20210616214526
20210616214135
20210609202149
20210526193025
20210507001928
20210505174616
20210502233513
20210425135952
20210421121431
20210416200740
20210324171613
20210323114347
20210322164601
20210308133107
20210306164523
20210221045109
20201218054746
20201206004238
20201017234926
20201017233919
20201008220312
20201008202037
20200917222734
20200917222316
20200917193528
20200917193034
20200917192924
20200908193330
20200630190544
20200630190240
20200628133322
20200627125810
20200622213645
20200620164023
20200614002136
20200608113046
20200605155027
20200601222558
20200529214050
20200521180606
20200518083523
20200516183822
20200516180352
20200510181721
20200510110808
20200508212852
20200417125749
20200407202420
20200407201300
20200317021758
20200312185443
20200312162302
20200312144258
20200309150742
20200306035625
20200126203551
20200119112504
20200114113335
20200113125135
20191218153258
20191212163405
20191212003415
20191031163205
20191007013357
20191001213028
20190927232842
20190927124642
20190917213523
20190915194355
20190914202517
20190904222339
20190901040524
20190901035623
20190823221802
20190820003045
20190819134503
20190815225426
20190807135426
20190805123746
20190729185330
20190726175042
20190715164535
20190715031050
20190706233204
20190705002136
20190701022101
20190627222826
20190627222225
20190529143559
20190519130537
20190511152737
20190511134027
20190509164208
20190420025523
20190409054914
20190403141604
20190317135723
20190316190352
20190314181829
20190307234537
20190306145741
20190304152020
20190226003449
20190225031625
20190225031541
20190203180359
20190201012802
20190117114553
20190103124754
20190103124649
20181226021420
20181219235220
20181213185533
20181213184704
20181207011115
20181204215309
20181204193439
20181203021853
20181203003808
20181127165847
20181127130500
20181116184611
20181116173541
20181116165755
20181026034033
20181024224956
20181018205649
20181017170937
20181010141500
20181007025445
20180929222014
20180831171112
20180820232245
20180814171349
20180813113448
20180812173710
20180812162710
20180812123222
20180808175627
20180711152640
20180707154237
20180628181026
20180617162849
20180616192031
20180615122121
20180609104432
20180608213548
20180528141303
20180514140000
20180514130000
20180510230049
20180510214435
20180506221944
20180416210259
20180410204633
20180402040909
20180402031200
20180310000000
20180304013859
20180211015820
20180206000000
20180204034416
20180109143959
20180106000232
20171226094803
20171212195226
20171201000000
20171130000000
20171129172043
20171125190735
20171125185353
20171125031751
20171125024930
20171122120436
20171119172437
20171118012443
20171116161857
20171114231651
20171114080328
20171109012327
20171107143624
20171107143332
20171028221157
20171020084748
20171010025614
20171010023049
20171006142024
20171005171936
20171005102658
20170928082043
20170927215609
20170924022025
20170920032311
20170920024819
20170918125918
20170917153509
20170913000752
20170905165803
20170905044538
20170901142658
20170901141119
20170829215220
20170824103029
20170823162448
20170720000000
20170718211102
20170716191202
20170714184731
20170713190709
20170713175513
20170713112503
20170711225116
20170625140443
20170624134742
20170623152212
20170610000000
20170609145826
20170606113804
20170604144747
20170601210557
20170520145338
20170516072309
20170508230434
20170507141759
20170507000211
20170506235850
20170427011934
20170425202925
20170425131920
20170424112722
20170424003227
20170423005413
20170418160728
20170414132105
20170414080609
20170409170753
20170406215816
20170405112956
20170403172249
20170330164118
20170330163835
20170330021336
20170322162804
20170322143850
20170322021028
20170318214217
20170317193015
20170304202101
20170303212857
20170301222600
20170217012631
20170214110202
20170209184350
20170205175257
20170127165745
20170125145934
20170123203248
20170123162658
20170119214911
20170114203041
20170114194937
20170112154826
20170109120109
20170105224407
20161222204147
20161222201034
20161221152630
20161205214545
20161203164520
20161202132159
20161130185319
20161130142058
20161128103007
20161123093447
20161122163057
20161119211120
20161116162355
20161105130633
20161104173623
20161027172456
20161009120834
20161006213403
20161003145426
20161003142332
20160926213048
20160920003904
20160919221059
20160905150353
20160826155805
20160325130944
20160322193748
20160316103650
20160314164231
20160312193225
20160306172223
20160305115639
20160227230233
20160224223247
20160223171800
20160223165855
20160223165723
20160223164502
20160223162837
20160222143943
20160222122600
20160221003621
20160221003140
20160220211917
20160220174730
\.


--
-- Data for Name: session_activations; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.session_activations (id, access_token_id, created_at, ip, session_id, updated_at, user_agent, user_id, web_push_subscription_id) FROM stdin;
\.


--
-- Data for Name: settings; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.settings (id, created_at, updated_at, value, var) FROM stdin;
9801	2026-07-01 15:14:00	2026-07-01 15:14:00	--- true\n	fixture_scalar
9802	2026-07-01 15:15:00	2026-07-01 15:15:00	--- !ruby/hash:ActiveSupport::HashWithIndifferentAccess\nfixture: value\n	fixture_tagged
\.


--
-- Data for Name: severed_relationships; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.severed_relationships (id, created_at, direction, languages, local_account_id, notify, relationship_severance_event_id, remote_account_id, show_reblogs, updated_at) FROM stdin;
8303	2026-07-01 17:00:00	0	{en}	116844606259201001	f	8301	116844606259202002	t	2026-07-01 17:00:00
\.


--
-- Data for Name: site_uploads; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.site_uploads (id, blurhash, created_at, file_content_type, file_file_name, file_file_size, file_updated_at, meta, updated_at, var) FROM stdin;
\.


--
-- Data for Name: software_updates; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.software_updates (id, created_at, release_notes, type, updated_at, urgent, version) FROM stdin;
\.


--
-- Data for Name: status_edits; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.status_edits (id, account_id, created_at, media_descriptions, ordered_media_attachment_ids, poll_options, quote_id, sensitive, spoiler_text, status_id, text, updated_at) FROM stdin;
9401	116844606259202001	2026-07-01 14:07:30	{}	{}	\N	\N	f		116845105643525105	Edited remote status before the current revision	2026-07-01 14:07:30
9402	116844606259201001	2026-07-01 19:15:00	\N	\N	\N	\N	f		116846257766400501	Deleted status edit must not be returned	2026-07-01 19:15:00
9403	116844606259201001	2026-07-01 13:00:30	{NULL,"Deterministic Mastodon test attachment"}	{-101,116844842188806001}	\N	\N	f		116844842188805001	Public status snapshot with a nullable media description	2026-07-01 13:00:30
\.


--
-- Data for Name: status_pins; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.status_pins (id, account_id, created_at, status_id, updated_at) FROM stdin;
9505	116844606259201001	2026-07-01 15:03:00	116844842188805001	2026-07-01 15:03:00
9508	116844606259201001	2026-07-01 19:10:00	116846257766400501	2026-07-01 19:10:00
\.


--
-- Data for Name: status_stats; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.status_stats (id, created_at, favourites_count, quotes_count, reblogs_count, replies_count, status_id, untrusted_favourites_count, untrusted_reblogs_count, updated_at) FROM stdin;
12002	2026-07-01 18:11:00	0	0	0	0	116844846120965002	\N	\N	2026-07-01 18:11:00
12003	2026-07-01 18:11:00	0	0	0	0	116844850053125003	\N	\N	2026-07-01 18:11:00
12005	2026-07-01 18:11:00	0	0	0	0	116844857917445005	\N	\N	2026-07-01 18:11:00
12006	2026-07-01 18:11:00	0	0	0	0	116845078118405101	\N	\N	2026-07-01 18:11:00
12008	2026-07-01 18:11:00	0	1	0	0	116845093847045103	\N	\N	2026-07-01 18:11:00
12009	2026-07-01 18:11:00	0	0	0	0	116845101711365104	\N	\N	2026-07-01 18:11:00
12010	2026-07-01 18:11:00	0	0	0	0	116845105643525105	\N	\N	2026-07-01 18:11:00
12011	2026-07-01 18:11:00	0	0	0	0	116845314048005201	\N	\N	2026-07-01 18:11:00
12012	2026-07-01 18:11:00	0	0	0	0	116845317980165202	\N	\N	2026-07-01 18:11:00
12013	2026-07-01 18:11:00	0	0	0	0	116845321912325301	\N	\N	2026-07-01 18:11:00
12001	2026-07-01 18:11:00	1	1	1	0	116844842188805001	\N	\N	2026-07-01 18:11:00
12004	2026-07-01 18:11:00	0	0	0	0	116844853985285004	\N	\N	2026-07-01 18:11:00
12007	2026-07-01 18:11:00	0	0	0	0	111680579174405102	\N	\N	2026-07-01 18:11:00
12014	2026-07-01 18:11:00	0	0	0	0	116846257766400501	\N	\N	2026-07-01 18:11:00
\.


--
-- Data for Name: status_trends; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.status_trends (id, account_id, allowed, language, rank, score, status_id) FROM stdin;
\.


--
-- Data for Name: statuses; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.statuses (id, account_id, application_id, conversation_id, created_at, deleted_at, edited_at, fetched_replies_at, in_reply_to_account_id, in_reply_to_id, language, local, ordered_media_attachment_ids, poll_id, quote_approval_policy, reblog_of_id, reply, sensitive, spoiler_text, text, trendable, updated_at, uri, url, visibility) FROM stdin;
116844846120965002	116844606259201001	\N	\N	2026-07-01 13:01:00	\N	\N	\N	\N	\N	en	t	\N	\N	0	\N	f	f		Unlisted fixture status	\N	2026-07-01 13:01:00	\N	\N	1
116844850053125003	116844606259201001	\N	\N	2026-07-01 13:02:00	\N	\N	\N	\N	\N	en	t	\N	\N	0	\N	f	f		Followers-only fixture status	\N	2026-07-01 13:02:00	\N	\N	2
116844857917445005	116844606259201001	\N	\N	2026-07-01 13:04:00	\N	\N	\N	\N	\N	en	t	\N	\N	0	\N	f	f		Limited fixture status	\N	2026-07-01 13:04:00	\N	\N	4
116845078118405101	116844606259202001	\N	\N	2026-07-01 14:00:00	\N	\N	\N	\N	\N	en	f	\N	\N	0	\N	f	f		Remote mention of @alice	\N	2026-07-01 14:00:00	https://remote.fixture.invalid/users/bob/statuses/116845078118405101	https://remote.fixture.invalid/@bob/116845078118405101	0
116845093847045103	116844606259202001	\N	\N	2026-07-01 14:04:00	\N	2026-07-01 14:05:00	\N	\N	\N	en	f	\N	\N	0	\N	f	f		Remote status quoted by Alice	\N	2026-07-01 14:05:00	https://remote.fixture.invalid/users/bob/statuses/116845093847045103	https://remote.fixture.invalid/@bob/116845093847045103	0
116845101711365104	116844606259202001	\N	\N	2026-07-01 14:06:00	\N	\N	\N	\N	\N	en	f	\N	\N	0	\N	f	f		New status notification activity	\N	2026-07-01 14:06:00	https://remote.fixture.invalid/users/bob/statuses/116845101711365104	https://remote.fixture.invalid/@bob/116845101711365104	0
116845105643525105	116844606259202001	\N	\N	2026-07-01 14:07:00	\N	2026-07-01 14:08:00	\N	\N	\N	en	f	\N	\N	0	\N	f	f		Edited remote status notification activity	\N	2026-07-01 14:08:00	https://remote.fixture.invalid/users/bob/statuses/116845105643525105	https://remote.fixture.invalid/@bob/116845105643525105	0
116845314048005201	116844606259201001	\N	\N	2026-07-01 15:00:00	\N	\N	\N	\N	\N	en	t	\N	\N	0	\N	f	f		Alice quotes Bob for quoted-update coverage	\N	2026-07-01 15:00:00	\N	\N	0
116845317980165202	116844606259202001	\N	\N	2026-07-01 15:01:00	\N	\N	\N	\N	\N	en	f	\N	\N	0	\N	f	f		Bob quotes Alice for quote coverage	\N	2026-07-01 15:01:00	https://remote.fixture.invalid/users/bob/statuses/116845317980165202	https://remote.fixture.invalid/@bob/116845317980165202	0
116845321912325301	116844606259202001	\N	\N	2026-07-01 15:02:00	\N	\N	\N	\N	\N	\N	f	\N	\N	0	116844842188805001	f	f			\N	2026-07-01 15:02:00	https://remote.fixture.invalid/users/bob/statuses/116845321912325301/activity	https://remote.fixture.invalid/@bob/116845321912325301	0
116844842188805001	116844606259201001	301	\N	2026-07-01 13:00:00	\N	\N	\N	\N	\N	en	t	{-101,116844842188806001,-102,-103,-104}	\N	2	\N	f	f		Public fixture status with local media	\N	2026-07-01 13:00:00	\N	\N	0
116844853985285004	116844606259201001	\N	9301	2026-07-01 13:03:00	\N	\N	\N	116844606259202001	116845078118405101	en	t	\N	\N	0	\N	t	f		Direct fixture status for Bob	\N	2026-07-01 13:03:00	\N	\N	3
111680579174405102	116844606259202001	\N	\N	2024-01-01 12:00:00	\N	\N	\N	\N	\N	en	f	\N	8201	0	\N	f	f		Historical fixture poll	\N	2024-01-02 12:00:00	https://remote.fixture.invalid/users/bob/statuses/111680579174405102	https://remote.fixture.invalid/@bob/111680579174405102	0
116846257766400501	116844606259201001	\N	\N	2026-07-01 19:00:00	2026-07-01 19:30:00	\N	\N	\N	\N	en	t	{}	8203	0	\N	f	f		Soft-deleted unknown visibility fixture status	\N	2026-07-01 19:30:00	\N	\N	99
\.


--
-- Data for Name: statuses_tags; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.statuses_tags (status_id, tag_id) FROM stdin;
116844842188805001	9201
116846257766400501	9201
\.


--
-- Data for Name: tag_follows; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.tag_follows (id, account_id, created_at, tag_id, updated_at) FROM stdin;
\.


--
-- Data for Name: tag_trends; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.tag_trends (id, allowed, language, rank, score, tag_id) FROM stdin;
\.


--
-- Data for Name: tagged_objects; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.tagged_objects (id, ap_type, created_at, object_id, object_type, status_id, updated_at, uri) FROM stdin;
\.


--
-- Data for Name: tags; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.tags (id, created_at, display_name, last_status_at, listable, max_score, max_score_at, name, requested_review_at, reviewed_at, trendable, updated_at, usable) FROM stdin;
9201	2026-07-01 13:00:00	FixtureTag	2026-07-01 13:00:00	t	\N	\N	fixturetag	\N	\N	f	2026-07-01 13:00:00	t
\.


--
-- Data for Name: terms_of_services; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.terms_of_services (id, changelog, created_at, effective_date, notification_sent_at, published_at, text, updated_at) FROM stdin;
\.


--
-- Data for Name: tombstones; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.tombstones (id, account_id, by_moderator, created_at, updated_at, uri) FROM stdin;
9901	116844606259202001	t	2026-07-01 16:12:00	2026-07-01 16:12:00	https://remote.fixture.invalid/users/bob/statuses/deleted-fixture
\.


--
-- Data for Name: unavailable_domains; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.unavailable_domains (id, created_at, domain, updated_at) FROM stdin;
\.


--
-- Data for Name: user_invite_requests; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.user_invite_requests (id, created_at, text, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: user_roles; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.user_roles (id, collection_limit, color, created_at, highlighted, name, permissions, "position", require_2fa, updated_at) FROM stdin;
-99	10		2026-07-01 12:00:00	f		1152921504606912512	-1	f	2026-07-01 12:00:00
91	10		2026-07-01 12:00:00	f	Fixture user	0	0	t	2026-07-01 12:00:00
92	10	1d9bf0	2026-07-01 12:00:00	t	Fixture moderator	1049616	10	t	2026-07-01 12:00:00
\.


--
-- Data for Name: username_blocks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.username_blocks (id, allow_with_approval, created_at, exact, normalized_username, updated_at, username) FROM stdin;
\.


--
-- Data for Name: users; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.users (id, account_id, age_verified_at, approved, chosen_languages, confirmation_sent_at, confirmation_token, confirmed_at, consumed_timestep, created_at, created_by_application_id, current_sign_in_at, disabled, email, encrypted_password, invite_id, last_emailed_at, last_sign_in_at, locale, otp_backup_codes, otp_required_for_login, otp_secret, require_tos_interstitial, reset_password_sent_at, reset_password_token, role_id, settings, sign_in_count, sign_in_token, sign_in_token_sent_at, sign_up_ip, skip_sign_in_token, time_zone, unconfirmed_email, updated_at, webauthn_id) FROM stdin;
102	116844606259201002	\N	t	{}	\N	\N	2026-07-01 12:00:00	\N	2026-07-01 12:00:00	\N	\N	f	moderator@fixture.invalid	$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC	\N	\N	\N	en	{}	f	\N	f	\N	\N	92	\N	0	\N	\N	192.0.2.11	\N	\N	\N	2026-07-01 12:00:00	\N
103	116844606259201003	\N	t	{en,fr}	\N	\N	2026-07-01 12:00:00	\N	2026-07-01 12:00:00	\N	\N	t	newbie@fixture.invalid	$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC	\N	\N	\N	en	\N	f	\N	f	\N	\N	91		0	\N	\N	192.0.2.12	\N	\N	\N	2026-07-01 12:00:00	\N
101	116844606259201001	\N	t	{en}	\N	\N	2026-07-01 12:00:00	\N	2026-07-01 12:00:00	\N	\N	f	alice@fixture.invalid	$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC	\N	\N	\N	en	{fixture-recovery-code}	t	\N	f	\N	\N	91	{"default_privacy":"private","nested":{"number":9007199254740993}}	0	\N	\N	192.0.2.0/24	\N	\N	\N	2026-07-01 12:00:00	fixture-alice-webauthn-id
\.


--
-- Data for Name: web_push_subscriptions; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.web_push_subscriptions (id, access_token_id, created_at, data, endpoint, key_auth, key_p256dh, standard, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: web_settings; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.web_settings (id, created_at, data, updated_at, user_id) FROM stdin;
\.


--
-- Data for Name: webauthn_credentials; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.webauthn_credentials (id, created_at, external_id, nickname, public_key, sign_count, updated_at, user_id) FROM stdin;
1	2026-07-01 12:00:00	fixture-alice-webauthn-credential	Fixture security key	fixture-public-credential-key	0	2026-07-01 12:00:00	101
\.


--
-- Data for Name: webhooks; Type: TABLE DATA; Schema: public; Owner: -
--

COPY public.webhooks (id, created_at, enabled, events, secret, template, updated_at, url) FROM stdin;
\.


--
-- Name: account_aliases_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_aliases_id_seq', 1, false);


--
-- Name: account_conversations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_conversations_id_seq', 9302, true);


--
-- Name: account_deletion_requests_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_deletion_requests_id_seq', 1, false);


--
-- Name: account_domain_blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_domain_blocks_id_seq', 9504, true);


--
-- Name: account_migrations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_migrations_id_seq', 1, false);


--
-- Name: account_moderation_notes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_moderation_notes_id_seq', 1, false);


--
-- Name: account_notes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_notes_id_seq', 1, false);


--
-- Name: account_pins_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_pins_id_seq', 1, false);


--
-- Name: account_relationship_severance_events_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_relationship_severance_events_id_seq', 8302, true);


--
-- Name: account_stats_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_stats_id_seq', 11007, true);


--
-- Name: account_statuses_cleanup_policies_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_statuses_cleanup_policies_id_seq', 1, false);


--
-- Name: account_warning_presets_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_warning_presets_id_seq', 1, false);


--
-- Name: account_warnings_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.account_warnings_id_seq', 8401, true);


--
-- Name: accounts_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.accounts_id_seq', 6, true);


--
-- Name: admin_action_logs_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.admin_action_logs_id_seq', 1, false);


--
-- Name: announcement_mutes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.announcement_mutes_id_seq', 1, false);


--
-- Name: announcement_reactions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.announcement_reactions_id_seq', 1, false);


--
-- Name: announcements_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.announcements_id_seq', 1, false);


--
-- Name: annual_report_statuses_per_account_counts_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.annual_report_statuses_per_account_counts_id_seq', 1, false);


--
-- Name: appeals_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.appeals_id_seq', 1, false);


--
-- Name: backups_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.backups_id_seq', 1, false);


--
-- Name: blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.blocks_id_seq', 9502, true);


--
-- Name: bookmarks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.bookmarks_id_seq', 9507, true);


--
-- Name: bulk_import_rows_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.bulk_import_rows_id_seq', 1, false);


--
-- Name: bulk_imports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.bulk_imports_id_seq', 1, false);


--
-- Name: canonical_email_blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.canonical_email_blocks_id_seq', 1, false);


--
-- Name: collection_items_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.collection_items_id_seq', 1, true);


--
-- Name: collection_reports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.collection_reports_id_seq', 1, false);


--
-- Name: collections_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.collections_id_seq', 1, true);


--
-- Name: conversation_mutes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.conversation_mutes_id_seq', 9303, true);


--
-- Name: conversations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.conversations_id_seq', 9301, true);


--
-- Name: custom_emoji_categories_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.custom_emoji_categories_id_seq', 1, false);


--
-- Name: custom_emojis_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.custom_emojis_id_seq', 1, false);


--
-- Name: custom_filter_keywords_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.custom_filter_keywords_id_seq', 9102, true);


--
-- Name: custom_filter_statuses_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.custom_filter_statuses_id_seq', 9104, true);


--
-- Name: custom_filters_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.custom_filters_id_seq', 9101, true);


--
-- Name: domain_allows_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.domain_allows_id_seq', 9601, true);


--
-- Name: domain_blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.domain_blocks_id_seq', 9602, true);


--
-- Name: email_domain_blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.email_domain_blocks_id_seq', 1, false);


--
-- Name: email_subscriptions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.email_subscriptions_id_seq', 1, false);


--
-- Name: fasp_backfill_requests_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.fasp_backfill_requests_id_seq', 1, false);


--
-- Name: fasp_debug_callbacks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.fasp_debug_callbacks_id_seq', 1, false);


--
-- Name: fasp_follow_recommendations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.fasp_follow_recommendations_id_seq', 1, false);


--
-- Name: fasp_providers_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.fasp_providers_id_seq', 1, false);


--
-- Name: fasp_subscriptions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.fasp_subscriptions_id_seq', 1, false);


--
-- Name: favourites_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.favourites_id_seq', 8102, true);


--
-- Name: featured_tags_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.featured_tags_id_seq', 9202, true);


--
-- Name: follow_recommendation_mutes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.follow_recommendation_mutes_id_seq', 1, false);


--
-- Name: follow_recommendation_suppressions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.follow_recommendation_suppressions_id_seq', 1, false);


--
-- Name: follow_requests_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.follow_requests_id_seq', 8003, true);


--
-- Name: follows_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.follows_id_seq', 8002, true);


--
-- Name: generated_annual_reports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.generated_annual_reports_id_seq', 8501, true);


--
-- Name: identities_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.identities_id_seq', 1, false);


--
-- Name: instance_moderation_notes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.instance_moderation_notes_id_seq', 1, false);


--
-- Name: invites_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.invites_id_seq', 1, false);


--
-- Name: ip_blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ip_blocks_id_seq', 1, false);


--
-- Name: keypairs_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.keypairs_id_seq', 8902, true);


--
-- Name: list_accounts_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.list_accounts_id_seq', 9004, true);


--
-- Name: lists_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.lists_id_seq', 9002, true);


--
-- Name: login_activities_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.login_activities_id_seq', 1, false);


--
-- Name: markers_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.markers_id_seq', 1, false);


--
-- Name: media_attachments_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.media_attachments_id_seq', 1, true);


--
-- Name: mentions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.mentions_id_seq', 7003, true);


--
-- Name: mutes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.mutes_id_seq', 9503, true);


--
-- Name: notification_permissions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.notification_permissions_id_seq', 9702, true);


--
-- Name: notification_policies_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.notification_policies_id_seq', 9701, true);


--
-- Name: notification_requests_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.notification_requests_id_seq', 1, true);


--
-- Name: notifications_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.notifications_id_seq', 10021, true);


--
-- Name: oauth_access_grants_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.oauth_access_grants_id_seq', 1, false);


--
-- Name: oauth_access_tokens_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.oauth_access_tokens_id_seq', 401, true);


--
-- Name: oauth_applications_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.oauth_applications_id_seq', 301, true);


--
-- Name: pghero_space_stats_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.pghero_space_stats_id_seq', 1, false);


--
-- Name: poll_votes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.poll_votes_id_seq', 8204, true);


--
-- Name: polls_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.polls_id_seq', 8203, true);


--
-- Name: preview_card_providers_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.preview_card_providers_id_seq', 1, false);


--
-- Name: preview_card_trends_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.preview_card_trends_id_seq', 1, false);


--
-- Name: preview_cards_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.preview_cards_id_seq', 1, false);


--
-- Name: quotes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.quotes_id_seq', 2, true);


--
-- Name: relationship_severance_events_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.relationship_severance_events_id_seq', 8301, true);


--
-- Name: relays_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.relays_id_seq', 1, false);


--
-- Name: report_notes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.report_notes_id_seq', 1, false);


--
-- Name: reports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.reports_id_seq', 8601, true);


--
-- Name: rule_translations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.rule_translations_id_seq', 1, false);


--
-- Name: rules_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.rules_id_seq', 1, false);


--
-- Name: scheduled_statuses_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.scheduled_statuses_id_seq', 1, false);


--
-- Name: session_activations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.session_activations_id_seq', 1, false);


--
-- Name: settings_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.settings_id_seq', 9802, true);


--
-- Name: severed_relationships_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.severed_relationships_id_seq', 8303, true);


--
-- Name: site_uploads_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.site_uploads_id_seq', 1, false);


--
-- Name: software_updates_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.software_updates_id_seq', 1, false);


--
-- Name: status_edits_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.status_edits_id_seq', 9403, true);


--
-- Name: status_pins_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.status_pins_id_seq', 9508, true);


--
-- Name: status_stats_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.status_stats_id_seq', 12014, true);


--
-- Name: status_trends_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.status_trends_id_seq', 1, false);


--
-- Name: statuses_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.statuses_id_seq', 14, true);


--
-- Name: tag_follows_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tag_follows_id_seq', 1, false);


--
-- Name: tag_trends_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tag_trends_id_seq', 1, false);


--
-- Name: tagged_objects_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tagged_objects_id_seq', 1, false);


--
-- Name: tags_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tags_id_seq', 9201, true);


--
-- Name: terms_of_services_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.terms_of_services_id_seq', 1, false);


--
-- Name: tombstones_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tombstones_id_seq', 9901, true);


--
-- Name: unavailable_domains_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.unavailable_domains_id_seq', 1, false);


--
-- Name: user_invite_requests_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.user_invite_requests_id_seq', 1, false);


--
-- Name: user_roles_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.user_roles_id_seq', 92, true);


--
-- Name: username_blocks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.username_blocks_id_seq', 1, false);


--
-- Name: users_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.users_id_seq', 103, true);


--
-- Name: web_push_subscriptions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.web_push_subscriptions_id_seq', 1, false);


--
-- Name: web_settings_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.web_settings_id_seq', 1, false);


--
-- Name: webauthn_credentials_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.webauthn_credentials_id_seq', 1, true);


--
-- Name: webhooks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.webhooks_id_seq', 1, false);


--
-- Name: account_aliases account_aliases_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_aliases
    ADD CONSTRAINT account_aliases_pkey PRIMARY KEY (id);


--
-- Name: account_conversations account_conversations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_conversations
    ADD CONSTRAINT account_conversations_pkey PRIMARY KEY (id);


--
-- Name: account_deletion_requests account_deletion_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_deletion_requests
    ADD CONSTRAINT account_deletion_requests_pkey PRIMARY KEY (id);


--
-- Name: account_domain_blocks account_domain_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_domain_blocks
    ADD CONSTRAINT account_domain_blocks_pkey PRIMARY KEY (id);


--
-- Name: account_migrations account_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_migrations
    ADD CONSTRAINT account_migrations_pkey PRIMARY KEY (id);


--
-- Name: account_moderation_notes account_moderation_notes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_moderation_notes
    ADD CONSTRAINT account_moderation_notes_pkey PRIMARY KEY (id);


--
-- Name: account_notes account_notes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_notes
    ADD CONSTRAINT account_notes_pkey PRIMARY KEY (id);


--
-- Name: account_pins account_pins_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_pins
    ADD CONSTRAINT account_pins_pkey PRIMARY KEY (id);


--
-- Name: account_relationship_severance_events account_relationship_severance_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_relationship_severance_events
    ADD CONSTRAINT account_relationship_severance_events_pkey PRIMARY KEY (id);


--
-- Name: account_stats account_stats_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_stats
    ADD CONSTRAINT account_stats_pkey PRIMARY KEY (id);


--
-- Name: account_statuses_cleanup_policies account_statuses_cleanup_policies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_statuses_cleanup_policies
    ADD CONSTRAINT account_statuses_cleanup_policies_pkey PRIMARY KEY (id);


--
-- Name: account_warning_presets account_warning_presets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warning_presets
    ADD CONSTRAINT account_warning_presets_pkey PRIMARY KEY (id);


--
-- Name: account_warnings account_warnings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warnings
    ADD CONSTRAINT account_warnings_pkey PRIMARY KEY (id);


--
-- Name: accounts accounts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_pkey PRIMARY KEY (id);


--
-- Name: accounts_tags accounts_tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts_tags
    ADD CONSTRAINT accounts_tags_pkey PRIMARY KEY (tag_id, account_id);


--
-- Name: admin_action_logs admin_action_logs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_action_logs
    ADD CONSTRAINT admin_action_logs_pkey PRIMARY KEY (id);


--
-- Name: announcement_mutes announcement_mutes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_mutes
    ADD CONSTRAINT announcement_mutes_pkey PRIMARY KEY (id);


--
-- Name: announcement_reactions announcement_reactions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_reactions
    ADD CONSTRAINT announcement_reactions_pkey PRIMARY KEY (id);


--
-- Name: announcements announcements_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcements
    ADD CONSTRAINT announcements_pkey PRIMARY KEY (id);


--
-- Name: annual_report_statuses_per_account_counts annual_report_statuses_per_account_counts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.annual_report_statuses_per_account_counts
    ADD CONSTRAINT annual_report_statuses_per_account_counts_pkey PRIMARY KEY (id);


--
-- Name: appeals appeals_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appeals
    ADD CONSTRAINT appeals_pkey PRIMARY KEY (id);


--
-- Name: ar_internal_metadata ar_internal_metadata_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ar_internal_metadata
    ADD CONSTRAINT ar_internal_metadata_pkey PRIMARY KEY (key);


--
-- Name: backups backups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.backups
    ADD CONSTRAINT backups_pkey PRIMARY KEY (id);


--
-- Name: blocks blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.blocks
    ADD CONSTRAINT blocks_pkey PRIMARY KEY (id);


--
-- Name: bookmarks bookmarks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_pkey PRIMARY KEY (id);


--
-- Name: bulk_import_rows bulk_import_rows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bulk_import_rows
    ADD CONSTRAINT bulk_import_rows_pkey PRIMARY KEY (id);


--
-- Name: bulk_imports bulk_imports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bulk_imports
    ADD CONSTRAINT bulk_imports_pkey PRIMARY KEY (id);


--
-- Name: canonical_email_blocks canonical_email_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.canonical_email_blocks
    ADD CONSTRAINT canonical_email_blocks_pkey PRIMARY KEY (id);


--
-- Name: collection_items collection_items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_items
    ADD CONSTRAINT collection_items_pkey PRIMARY KEY (id);


--
-- Name: collection_reports collection_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_reports
    ADD CONSTRAINT collection_reports_pkey PRIMARY KEY (id);


--
-- Name: collections collections_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collections
    ADD CONSTRAINT collections_pkey PRIMARY KEY (id);


--
-- Name: conversation_mutes conversation_mutes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_mutes
    ADD CONSTRAINT conversation_mutes_pkey PRIMARY KEY (id);


--
-- Name: conversations conversations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_pkey PRIMARY KEY (id);


--
-- Name: custom_emoji_categories custom_emoji_categories_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emoji_categories
    ADD CONSTRAINT custom_emoji_categories_pkey PRIMARY KEY (id);


--
-- Name: custom_emojis custom_emojis_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emojis
    ADD CONSTRAINT custom_emojis_pkey PRIMARY KEY (id);


--
-- Name: custom_filter_keywords custom_filter_keywords_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_keywords
    ADD CONSTRAINT custom_filter_keywords_pkey PRIMARY KEY (id);


--
-- Name: custom_filter_statuses custom_filter_statuses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_statuses
    ADD CONSTRAINT custom_filter_statuses_pkey PRIMARY KEY (id);


--
-- Name: custom_filters custom_filters_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filters
    ADD CONSTRAINT custom_filters_pkey PRIMARY KEY (id);


--
-- Name: domain_allows domain_allows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.domain_allows
    ADD CONSTRAINT domain_allows_pkey PRIMARY KEY (id);


--
-- Name: domain_blocks domain_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.domain_blocks
    ADD CONSTRAINT domain_blocks_pkey PRIMARY KEY (id);


--
-- Name: email_domain_blocks email_domain_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_domain_blocks
    ADD CONSTRAINT email_domain_blocks_pkey PRIMARY KEY (id);


--
-- Name: email_subscriptions email_subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_subscriptions
    ADD CONSTRAINT email_subscriptions_pkey PRIMARY KEY (id);


--
-- Name: fasp_backfill_requests fasp_backfill_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_backfill_requests
    ADD CONSTRAINT fasp_backfill_requests_pkey PRIMARY KEY (id);


--
-- Name: fasp_debug_callbacks fasp_debug_callbacks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_debug_callbacks
    ADD CONSTRAINT fasp_debug_callbacks_pkey PRIMARY KEY (id);


--
-- Name: fasp_follow_recommendations fasp_follow_recommendations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_follow_recommendations
    ADD CONSTRAINT fasp_follow_recommendations_pkey PRIMARY KEY (id);


--
-- Name: fasp_providers fasp_providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_providers
    ADD CONSTRAINT fasp_providers_pkey PRIMARY KEY (id);


--
-- Name: fasp_subscriptions fasp_subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_subscriptions
    ADD CONSTRAINT fasp_subscriptions_pkey PRIMARY KEY (id);


--
-- Name: favourites favourites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favourites
    ADD CONSTRAINT favourites_pkey PRIMARY KEY (id);


--
-- Name: featured_tags featured_tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.featured_tags
    ADD CONSTRAINT featured_tags_pkey PRIMARY KEY (id);


--
-- Name: follow_recommendation_mutes follow_recommendation_mutes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_mutes
    ADD CONSTRAINT follow_recommendation_mutes_pkey PRIMARY KEY (id);


--
-- Name: follow_recommendation_suppressions follow_recommendation_suppressions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_suppressions
    ADD CONSTRAINT follow_recommendation_suppressions_pkey PRIMARY KEY (id);


--
-- Name: follow_requests follow_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_requests
    ADD CONSTRAINT follow_requests_pkey PRIMARY KEY (id);


--
-- Name: follows follows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follows
    ADD CONSTRAINT follows_pkey PRIMARY KEY (id);


--
-- Name: generated_annual_reports generated_annual_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.generated_annual_reports
    ADD CONSTRAINT generated_annual_reports_pkey PRIMARY KEY (id);


--
-- Name: identities identities_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identities
    ADD CONSTRAINT identities_pkey PRIMARY KEY (id);


--
-- Name: instance_moderation_notes instance_moderation_notes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.instance_moderation_notes
    ADD CONSTRAINT instance_moderation_notes_pkey PRIMARY KEY (id);


--
-- Name: invites invites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_pkey PRIMARY KEY (id);


--
-- Name: ip_blocks ip_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ip_blocks
    ADD CONSTRAINT ip_blocks_pkey PRIMARY KEY (id);


--
-- Name: keypairs keypairs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.keypairs
    ADD CONSTRAINT keypairs_pkey PRIMARY KEY (id);


--
-- Name: list_accounts list_accounts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.list_accounts
    ADD CONSTRAINT list_accounts_pkey PRIMARY KEY (id);


--
-- Name: lists lists_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.lists
    ADD CONSTRAINT lists_pkey PRIMARY KEY (id);


--
-- Name: login_activities login_activities_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.login_activities
    ADD CONSTRAINT login_activities_pkey PRIMARY KEY (id);


--
-- Name: markers markers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.markers
    ADD CONSTRAINT markers_pkey PRIMARY KEY (id);


--
-- Name: media_attachments media_attachments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.media_attachments
    ADD CONSTRAINT media_attachments_pkey PRIMARY KEY (id);


--
-- Name: mentions mentions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mentions
    ADD CONSTRAINT mentions_pkey PRIMARY KEY (id);


--
-- Name: mutes mutes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mutes
    ADD CONSTRAINT mutes_pkey PRIMARY KEY (id);


--
-- Name: notification_permissions notification_permissions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_permissions
    ADD CONSTRAINT notification_permissions_pkey PRIMARY KEY (id);


--
-- Name: notification_policies notification_policies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_policies
    ADD CONSTRAINT notification_policies_pkey PRIMARY KEY (id);


--
-- Name: notification_requests notification_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_requests
    ADD CONSTRAINT notification_requests_pkey PRIMARY KEY (id);


--
-- Name: notifications notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);


--
-- Name: oauth_access_grants oauth_access_grants_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_grants
    ADD CONSTRAINT oauth_access_grants_pkey PRIMARY KEY (id);


--
-- Name: oauth_access_tokens oauth_access_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_tokens
    ADD CONSTRAINT oauth_access_tokens_pkey PRIMARY KEY (id);


--
-- Name: oauth_applications oauth_applications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_applications
    ADD CONSTRAINT oauth_applications_pkey PRIMARY KEY (id);


--
-- Name: pghero_space_stats pghero_space_stats_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pghero_space_stats
    ADD CONSTRAINT pghero_space_stats_pkey PRIMARY KEY (id);


--
-- Name: poll_votes poll_votes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT poll_votes_pkey PRIMARY KEY (id);


--
-- Name: polls polls_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.polls
    ADD CONSTRAINT polls_pkey PRIMARY KEY (id);


--
-- Name: preview_card_providers preview_card_providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_card_providers
    ADD CONSTRAINT preview_card_providers_pkey PRIMARY KEY (id);


--
-- Name: preview_card_trends preview_card_trends_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_card_trends
    ADD CONSTRAINT preview_card_trends_pkey PRIMARY KEY (id);


--
-- Name: preview_cards preview_cards_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_cards
    ADD CONSTRAINT preview_cards_pkey PRIMARY KEY (id);


--
-- Name: preview_cards_statuses preview_cards_statuses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_cards_statuses
    ADD CONSTRAINT preview_cards_statuses_pkey PRIMARY KEY (status_id, preview_card_id);


--
-- Name: quotes quotes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.quotes
    ADD CONSTRAINT quotes_pkey PRIMARY KEY (id);


--
-- Name: relationship_severance_events relationship_severance_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.relationship_severance_events
    ADD CONSTRAINT relationship_severance_events_pkey PRIMARY KEY (id);


--
-- Name: relays relays_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.relays
    ADD CONSTRAINT relays_pkey PRIMARY KEY (id);


--
-- Name: report_notes report_notes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_notes
    ADD CONSTRAINT report_notes_pkey PRIMARY KEY (id);


--
-- Name: reports reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);


--
-- Name: rule_translations rule_translations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rule_translations
    ADD CONSTRAINT rule_translations_pkey PRIMARY KEY (id);


--
-- Name: rules rules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rules
    ADD CONSTRAINT rules_pkey PRIMARY KEY (id);


--
-- Name: scheduled_statuses scheduled_statuses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scheduled_statuses
    ADD CONSTRAINT scheduled_statuses_pkey PRIMARY KEY (id);


--
-- Name: schema_migrations schema_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (version);


--
-- Name: session_activations session_activations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.session_activations
    ADD CONSTRAINT session_activations_pkey PRIMARY KEY (id);


--
-- Name: settings settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings
    ADD CONSTRAINT settings_pkey PRIMARY KEY (id);


--
-- Name: severed_relationships severed_relationships_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.severed_relationships
    ADD CONSTRAINT severed_relationships_pkey PRIMARY KEY (id);


--
-- Name: site_uploads site_uploads_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.site_uploads
    ADD CONSTRAINT site_uploads_pkey PRIMARY KEY (id);


--
-- Name: software_updates software_updates_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.software_updates
    ADD CONSTRAINT software_updates_pkey PRIMARY KEY (id);


--
-- Name: status_edits status_edits_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_edits
    ADD CONSTRAINT status_edits_pkey PRIMARY KEY (id);


--
-- Name: status_pins status_pins_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_pins
    ADD CONSTRAINT status_pins_pkey PRIMARY KEY (id);


--
-- Name: status_stats status_stats_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_stats
    ADD CONSTRAINT status_stats_pkey PRIMARY KEY (id);


--
-- Name: status_trends status_trends_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_trends
    ADD CONSTRAINT status_trends_pkey PRIMARY KEY (id);


--
-- Name: statuses statuses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses
    ADD CONSTRAINT statuses_pkey PRIMARY KEY (id);


--
-- Name: statuses_tags statuses_tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses_tags
    ADD CONSTRAINT statuses_tags_pkey PRIMARY KEY (tag_id, status_id);


--
-- Name: tag_follows tag_follows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_follows
    ADD CONSTRAINT tag_follows_pkey PRIMARY KEY (id);


--
-- Name: tag_trends tag_trends_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_trends
    ADD CONSTRAINT tag_trends_pkey PRIMARY KEY (id);


--
-- Name: tagged_objects tagged_objects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tagged_objects
    ADD CONSTRAINT tagged_objects_pkey PRIMARY KEY (id);


--
-- Name: tags tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags
    ADD CONSTRAINT tags_pkey PRIMARY KEY (id);


--
-- Name: terms_of_services terms_of_services_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.terms_of_services
    ADD CONSTRAINT terms_of_services_pkey PRIMARY KEY (id);


--
-- Name: tombstones tombstones_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tombstones
    ADD CONSTRAINT tombstones_pkey PRIMARY KEY (id);


--
-- Name: unavailable_domains unavailable_domains_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unavailable_domains
    ADD CONSTRAINT unavailable_domains_pkey PRIMARY KEY (id);


--
-- Name: user_invite_requests user_invite_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_invite_requests
    ADD CONSTRAINT user_invite_requests_pkey PRIMARY KEY (id);


--
-- Name: user_roles user_roles_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_roles
    ADD CONSTRAINT user_roles_pkey PRIMARY KEY (id);


--
-- Name: username_blocks username_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.username_blocks
    ADD CONSTRAINT username_blocks_pkey PRIMARY KEY (id);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: web_push_subscriptions web_push_subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_push_subscriptions
    ADD CONSTRAINT web_push_subscriptions_pkey PRIMARY KEY (id);


--
-- Name: web_settings web_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_settings
    ADD CONSTRAINT web_settings_pkey PRIMARY KEY (id);


--
-- Name: webauthn_credentials webauthn_credentials_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_pkey PRIMARY KEY (id);


--
-- Name: webhooks webhooks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhooks
    ADD CONSTRAINT webhooks_pkey PRIMARY KEY (id);


--
-- Name: idx_on_account_id_language_sensitive_250461e1eb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_on_account_id_language_sensitive_250461e1eb ON public.account_summaries USING btree (account_id, language, sensitive);


--
-- Name: idx_on_account_id_relationship_severance_event_id_7bd82bf20e; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_on_account_id_relationship_severance_event_id_7bd82bf20e ON public.account_relationship_severance_events USING btree (account_id, relationship_severance_event_id);


--
-- Name: idx_on_account_id_target_account_id_a8c8ddf44e; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_on_account_id_target_account_id_a8c8ddf44e ON public.follow_recommendation_mutes USING btree (account_id, target_account_id);


--
-- Name: idx_on_relationship_severance_event_id_403f53e707; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_on_relationship_severance_event_id_403f53e707 ON public.account_relationship_severance_events USING btree (relationship_severance_event_id);


--
-- Name: idx_on_status_id_object_type_object_id_d6ebe374bd; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_on_status_id_object_type_object_id_d6ebe374bd ON public.tagged_objects USING btree (status_id, object_type, object_id) WHERE ((object_type IS NOT NULL) AND (object_id IS NOT NULL));


--
-- Name: idx_on_year_account_id_ff3e167cef; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_on_year_account_id_ff3e167cef ON public.annual_report_statuses_per_account_counts USING btree (year, account_id);


--
-- Name: index_account_aliases_on_account_id_and_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_account_aliases_on_account_id_and_uri ON public.account_aliases USING btree (account_id, uri);


--
-- Name: index_account_conversations_on_conversation_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_conversations_on_conversation_id ON public.account_conversations USING btree (conversation_id);


--
-- Name: index_account_deletion_requests_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_deletion_requests_on_account_id ON public.account_deletion_requests USING btree (account_id);


--
-- Name: index_account_domain_blocks_on_account_id_and_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_account_domain_blocks_on_account_id_and_domain ON public.account_domain_blocks USING btree (account_id, domain);


--
-- Name: index_account_migrations_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_migrations_on_account_id ON public.account_migrations USING btree (account_id);


--
-- Name: index_account_migrations_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_migrations_on_target_account_id ON public.account_migrations USING btree (target_account_id) WHERE (target_account_id IS NOT NULL);


--
-- Name: index_account_moderation_notes_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_moderation_notes_on_account_id ON public.account_moderation_notes USING btree (account_id);


--
-- Name: index_account_moderation_notes_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_moderation_notes_on_target_account_id ON public.account_moderation_notes USING btree (target_account_id);


--
-- Name: index_account_notes_on_account_id_and_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_account_notes_on_account_id_and_target_account_id ON public.account_notes USING btree (account_id, target_account_id);


--
-- Name: index_account_notes_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_notes_on_target_account_id ON public.account_notes USING btree (target_account_id);


--
-- Name: index_account_pins_on_account_id_and_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_account_pins_on_account_id_and_target_account_id ON public.account_pins USING btree (account_id, target_account_id);


--
-- Name: index_account_pins_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_pins_on_target_account_id ON public.account_pins USING btree (target_account_id);


--
-- Name: index_account_stats_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_account_stats_on_account_id ON public.account_stats USING btree (account_id);


--
-- Name: index_account_stats_on_last_status_at_and_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_stats_on_last_status_at_and_account_id ON public.account_stats USING btree (last_status_at DESC NULLS LAST, account_id);


--
-- Name: index_account_statuses_cleanup_policies_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_statuses_cleanup_policies_on_account_id ON public.account_statuses_cleanup_policies USING btree (account_id);


--
-- Name: index_account_summaries_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_account_summaries_on_account_id ON public.account_summaries USING btree (account_id);


--
-- Name: index_account_warnings_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_warnings_on_account_id ON public.account_warnings USING btree (account_id);


--
-- Name: index_account_warnings_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_account_warnings_on_target_account_id ON public.account_warnings USING btree (target_account_id);


--
-- Name: index_accounts_on_domain_and_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_accounts_on_domain_and_id ON public.accounts USING btree (domain, id);


--
-- Name: index_accounts_on_moved_to_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_accounts_on_moved_to_account_id ON public.accounts USING btree (moved_to_account_id) WHERE (moved_to_account_id IS NOT NULL);


--
-- Name: index_accounts_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_accounts_on_uri ON public.accounts USING btree (uri);


--
-- Name: index_accounts_on_url; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_accounts_on_url ON public.accounts USING btree (url text_pattern_ops) WHERE (url IS NOT NULL);


--
-- Name: index_accounts_on_username_and_domain_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_accounts_on_username_and_domain_lower ON public.accounts USING btree (lower((username)::text), COALESCE(lower((domain)::text), ''::text));


--
-- Name: index_accounts_tags_on_account_id_and_tag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_accounts_tags_on_account_id_and_tag_id ON public.accounts_tags USING btree (account_id, tag_id);


--
-- Name: index_admin_action_logs_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_admin_action_logs_on_account_id ON public.admin_action_logs USING btree (account_id);


--
-- Name: index_admin_action_logs_on_target_type_and_target_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_admin_action_logs_on_target_type_and_target_id ON public.admin_action_logs USING btree (target_type, target_id);


--
-- Name: index_announcement_mutes_on_account_id_and_announcement_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_announcement_mutes_on_account_id_and_announcement_id ON public.announcement_mutes USING btree (account_id, announcement_id);


--
-- Name: index_announcement_mutes_on_announcement_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_announcement_mutes_on_announcement_id ON public.announcement_mutes USING btree (announcement_id);


--
-- Name: index_announcement_reactions_on_account_id_and_announcement_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_announcement_reactions_on_account_id_and_announcement_id ON public.announcement_reactions USING btree (account_id, announcement_id, name);


--
-- Name: index_announcement_reactions_on_announcement_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_announcement_reactions_on_announcement_id ON public.announcement_reactions USING btree (announcement_id);


--
-- Name: index_announcement_reactions_on_custom_emoji_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_announcement_reactions_on_custom_emoji_id ON public.announcement_reactions USING btree (custom_emoji_id) WHERE (custom_emoji_id IS NOT NULL);


--
-- Name: index_appeals_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_appeals_on_account_id ON public.appeals USING btree (account_id);


--
-- Name: index_appeals_on_account_warning_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_appeals_on_account_warning_id ON public.appeals USING btree (account_warning_id);


--
-- Name: index_appeals_on_approved_by_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_appeals_on_approved_by_account_id ON public.appeals USING btree (approved_by_account_id) WHERE (approved_by_account_id IS NOT NULL);


--
-- Name: index_appeals_on_rejected_by_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_appeals_on_rejected_by_account_id ON public.appeals USING btree (rejected_by_account_id) WHERE (rejected_by_account_id IS NOT NULL);


--
-- Name: index_backups_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_backups_on_user_id ON public.backups USING btree (user_id);


--
-- Name: index_blocks_on_account_id_and_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_blocks_on_account_id_and_target_account_id ON public.blocks USING btree (account_id, target_account_id);


--
-- Name: index_blocks_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_blocks_on_target_account_id ON public.blocks USING btree (target_account_id);


--
-- Name: index_bookmarks_on_account_id_and_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_bookmarks_on_account_id_and_status_id ON public.bookmarks USING btree (account_id, status_id);


--
-- Name: index_bookmarks_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_bookmarks_on_status_id ON public.bookmarks USING btree (status_id);


--
-- Name: index_bulk_import_rows_on_bulk_import_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_bulk_import_rows_on_bulk_import_id ON public.bulk_import_rows USING btree (bulk_import_id);


--
-- Name: index_bulk_imports_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_bulk_imports_on_account_id ON public.bulk_imports USING btree (account_id);


--
-- Name: index_bulk_imports_unconfirmed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_bulk_imports_unconfirmed ON public.bulk_imports USING btree (id) WHERE (state = 0);


--
-- Name: index_canonical_email_blocks_on_canonical_email_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_canonical_email_blocks_on_canonical_email_hash ON public.canonical_email_blocks USING btree (canonical_email_hash);


--
-- Name: index_canonical_email_blocks_on_reference_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_canonical_email_blocks_on_reference_account_id ON public.canonical_email_blocks USING btree (reference_account_id);


--
-- Name: index_collection_items_on_account_id_and_collection_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_collection_items_on_account_id_and_collection_id ON public.collection_items USING btree (account_id, collection_id);


--
-- Name: index_collection_items_on_approval_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_collection_items_on_approval_uri ON public.collection_items USING btree (approval_uri) WHERE (approval_uri IS NOT NULL);


--
-- Name: index_collection_items_on_collection_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_collection_items_on_collection_id ON public.collection_items USING btree (collection_id);


--
-- Name: index_collection_items_on_state; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_collection_items_on_state ON public.collection_items USING btree (state) WHERE (state = ANY (ARRAY[2, 3]));


--
-- Name: index_collection_items_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_collection_items_on_uri ON public.collection_items USING btree (uri) WHERE (uri IS NOT NULL);


--
-- Name: index_collection_reports_on_collection_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_collection_reports_on_collection_id ON public.collection_reports USING btree (collection_id);


--
-- Name: index_collection_reports_on_report_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_collection_reports_on_report_id ON public.collection_reports USING btree (report_id);


--
-- Name: index_collections_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_collections_on_account_id ON public.collections USING btree (account_id);


--
-- Name: index_collections_on_tag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_collections_on_tag_id ON public.collections USING btree (tag_id);


--
-- Name: index_collections_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_collections_on_uri ON public.collections USING btree (uri) WHERE (uri IS NOT NULL);


--
-- Name: index_conversation_mutes_on_account_id_and_conversation_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_conversation_mutes_on_account_id_and_conversation_id ON public.conversation_mutes USING btree (account_id, conversation_id);


--
-- Name: index_conversations_on_parent_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_conversations_on_parent_status_id ON public.conversations USING btree (parent_status_id) WHERE (parent_status_id IS NOT NULL);


--
-- Name: index_conversations_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_conversations_on_uri ON public.conversations USING btree (uri text_pattern_ops) WHERE (uri IS NOT NULL);


--
-- Name: index_custom_emoji_categories_on_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_custom_emoji_categories_on_name ON public.custom_emoji_categories USING btree (name);


--
-- Name: index_custom_emojis_on_shortcode_and_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_custom_emojis_on_shortcode_and_domain ON public.custom_emojis USING btree (shortcode, domain);


--
-- Name: index_custom_filter_keywords_on_custom_filter_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_custom_filter_keywords_on_custom_filter_id ON public.custom_filter_keywords USING btree (custom_filter_id);


--
-- Name: index_custom_filter_statuses_on_custom_filter_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_custom_filter_statuses_on_custom_filter_id ON public.custom_filter_statuses USING btree (custom_filter_id);


--
-- Name: index_custom_filter_statuses_on_status_id_and_custom_filter_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_custom_filter_statuses_on_status_id_and_custom_filter_id ON public.custom_filter_statuses USING btree (status_id, custom_filter_id);


--
-- Name: index_custom_filters_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_custom_filters_on_account_id ON public.custom_filters USING btree (account_id);


--
-- Name: index_domain_allows_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_domain_allows_on_domain ON public.domain_allows USING btree (domain);


--
-- Name: index_domain_blocks_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_domain_blocks_on_domain ON public.domain_blocks USING btree (domain);


--
-- Name: index_email_domain_blocks_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_email_domain_blocks_on_domain ON public.email_domain_blocks USING btree (domain);


--
-- Name: index_email_subscriptions_on_account_id_and_email; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_email_subscriptions_on_account_id_and_email ON public.email_subscriptions USING btree (account_id, email);


--
-- Name: index_email_subscriptions_on_confirmation_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_email_subscriptions_on_confirmation_token ON public.email_subscriptions USING btree (confirmation_token) WHERE (confirmation_token IS NOT NULL);


--
-- Name: index_fasp_backfill_requests_on_fasp_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_fasp_backfill_requests_on_fasp_provider_id ON public.fasp_backfill_requests USING btree (fasp_provider_id);


--
-- Name: index_fasp_debug_callbacks_on_fasp_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_fasp_debug_callbacks_on_fasp_provider_id ON public.fasp_debug_callbacks USING btree (fasp_provider_id);


--
-- Name: index_fasp_follow_recommendations_on_recommended_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_fasp_follow_recommendations_on_recommended_account_id ON public.fasp_follow_recommendations USING btree (recommended_account_id);


--
-- Name: index_fasp_follow_recommendations_on_requesting_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_fasp_follow_recommendations_on_requesting_account_id ON public.fasp_follow_recommendations USING btree (requesting_account_id);


--
-- Name: index_fasp_providers_on_base_url; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_fasp_providers_on_base_url ON public.fasp_providers USING btree (base_url);


--
-- Name: index_fasp_subscriptions_on_fasp_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_fasp_subscriptions_on_fasp_provider_id ON public.fasp_subscriptions USING btree (fasp_provider_id);


--
-- Name: index_favourites_on_account_id_and_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_favourites_on_account_id_and_id ON public.favourites USING btree (account_id, id);


--
-- Name: index_favourites_on_account_id_and_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_favourites_on_account_id_and_status_id ON public.favourites USING btree (account_id, status_id);


--
-- Name: index_favourites_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_favourites_on_status_id ON public.favourites USING btree (status_id);


--
-- Name: index_featured_tags_on_account_id_and_tag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_featured_tags_on_account_id_and_tag_id ON public.featured_tags USING btree (account_id, tag_id);


--
-- Name: index_featured_tags_on_tag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_featured_tags_on_tag_id ON public.featured_tags USING btree (tag_id);


--
-- Name: index_follow_recommendation_mutes_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_follow_recommendation_mutes_on_target_account_id ON public.follow_recommendation_mutes USING btree (target_account_id);


--
-- Name: index_follow_recommendation_suppressions_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_follow_recommendation_suppressions_on_account_id ON public.follow_recommendation_suppressions USING btree (account_id);


--
-- Name: index_follow_requests_on_account_id_and_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_follow_requests_on_account_id_and_target_account_id ON public.follow_requests USING btree (account_id, target_account_id);


--
-- Name: index_follows_on_account_id_and_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_follows_on_account_id_and_target_account_id ON public.follows USING btree (account_id, target_account_id);


--
-- Name: index_follows_on_target_account_id_and_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_follows_on_target_account_id_and_account_id ON public.follows USING btree (target_account_id, account_id);


--
-- Name: index_generated_annual_reports_on_account_id_and_year; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_generated_annual_reports_on_account_id_and_year ON public.generated_annual_reports USING btree (account_id, year);


--
-- Name: index_global_follow_recommendations_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_global_follow_recommendations_on_account_id ON public.global_follow_recommendations USING btree (account_id);


--
-- Name: index_identities_on_uid_and_provider; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_identities_on_uid_and_provider ON public.identities USING btree (uid, provider);


--
-- Name: index_identities_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_identities_on_user_id ON public.identities USING btree (user_id);


--
-- Name: index_instance_moderation_notes_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_instance_moderation_notes_on_domain ON public.instance_moderation_notes USING btree (domain);


--
-- Name: index_instances_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_instances_on_domain ON public.instances USING btree (domain);


--
-- Name: index_instances_on_reverse_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_instances_on_reverse_domain ON public.instances USING btree (reverse(('.'::text || (domain)::text)), domain);


--
-- Name: index_invites_on_code; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_invites_on_code ON public.invites USING btree (code);


--
-- Name: index_invites_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_invites_on_user_id ON public.invites USING btree (user_id);


--
-- Name: index_ip_blocks_on_ip; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_ip_blocks_on_ip ON public.ip_blocks USING btree (ip);


--
-- Name: index_keypairs_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_keypairs_on_account_id ON public.keypairs USING btree (account_id);


--
-- Name: index_keypairs_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_keypairs_on_uri ON public.keypairs USING btree (uri);


--
-- Name: index_list_accounts_on_account_id_and_list_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_list_accounts_on_account_id_and_list_id ON public.list_accounts USING btree (account_id, list_id);


--
-- Name: index_list_accounts_on_follow_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_list_accounts_on_follow_id ON public.list_accounts USING btree (follow_id) WHERE (follow_id IS NOT NULL);


--
-- Name: index_list_accounts_on_follow_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_list_accounts_on_follow_request_id ON public.list_accounts USING btree (follow_request_id) WHERE (follow_request_id IS NOT NULL);


--
-- Name: index_list_accounts_on_list_id_and_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_list_accounts_on_list_id_and_account_id ON public.list_accounts USING btree (list_id, account_id);


--
-- Name: index_lists_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_lists_on_account_id ON public.lists USING btree (account_id);


--
-- Name: index_login_activities_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_login_activities_on_user_id ON public.login_activities USING btree (user_id);


--
-- Name: index_markers_on_user_id_and_timeline; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_markers_on_user_id_and_timeline ON public.markers USING btree (user_id, timeline);


--
-- Name: index_media_attachments_on_account_id_and_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_media_attachments_on_account_id_and_status_id ON public.media_attachments USING btree (account_id, status_id DESC);


--
-- Name: index_media_attachments_on_scheduled_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_media_attachments_on_scheduled_status_id ON public.media_attachments USING btree (scheduled_status_id) WHERE (scheduled_status_id IS NOT NULL);


--
-- Name: index_media_attachments_on_shortcode; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_media_attachments_on_shortcode ON public.media_attachments USING btree (shortcode text_pattern_ops) WHERE (shortcode IS NOT NULL);


--
-- Name: index_media_attachments_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_media_attachments_on_status_id ON public.media_attachments USING btree (status_id);


--
-- Name: index_mentions_on_account_id_and_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_mentions_on_account_id_and_status_id ON public.mentions USING btree (account_id, status_id);


--
-- Name: index_mentions_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_mentions_on_status_id ON public.mentions USING btree (status_id);


--
-- Name: index_mutes_on_account_id_and_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_mutes_on_account_id_and_target_account_id ON public.mutes USING btree (account_id, target_account_id);


--
-- Name: index_mutes_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_mutes_on_target_account_id ON public.mutes USING btree (target_account_id);


--
-- Name: index_notification_permissions_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notification_permissions_on_account_id ON public.notification_permissions USING btree (account_id);


--
-- Name: index_notification_permissions_on_from_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notification_permissions_on_from_account_id ON public.notification_permissions USING btree (from_account_id);


--
-- Name: index_notification_policies_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_notification_policies_on_account_id ON public.notification_policies USING btree (account_id);


--
-- Name: index_notification_requests_on_account_id_and_from_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_notification_requests_on_account_id_and_from_account_id ON public.notification_requests USING btree (account_id, from_account_id);


--
-- Name: index_notification_requests_on_from_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notification_requests_on_from_account_id ON public.notification_requests USING btree (from_account_id);


--
-- Name: index_notification_requests_on_last_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notification_requests_on_last_status_id ON public.notification_requests USING btree (last_status_id);


--
-- Name: index_notifications_on_account_id_and_group_key; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notifications_on_account_id_and_group_key ON public.notifications USING btree (account_id, group_key) WHERE (group_key IS NOT NULL);


--
-- Name: index_notifications_on_account_id_and_id_and_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notifications_on_account_id_and_id_and_type ON public.notifications USING btree (account_id, id DESC, type);


--
-- Name: index_notifications_on_activity_id_and_activity_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notifications_on_activity_id_and_activity_type ON public.notifications USING btree (activity_id, activity_type);


--
-- Name: index_notifications_on_filtered; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notifications_on_filtered ON public.notifications USING btree (account_id, id DESC, type) WHERE (filtered = false);


--
-- Name: index_notifications_on_from_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_notifications_on_from_account_id ON public.notifications USING btree (from_account_id);


--
-- Name: index_oauth_access_grants_on_resource_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_oauth_access_grants_on_resource_owner_id ON public.oauth_access_grants USING btree (resource_owner_id);


--
-- Name: index_oauth_access_grants_on_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_oauth_access_grants_on_token ON public.oauth_access_grants USING btree (token);


--
-- Name: index_oauth_access_tokens_on_refresh_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_oauth_access_tokens_on_refresh_token ON public.oauth_access_tokens USING btree (refresh_token text_pattern_ops) WHERE (refresh_token IS NOT NULL);


--
-- Name: index_oauth_access_tokens_on_resource_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_oauth_access_tokens_on_resource_owner_id ON public.oauth_access_tokens USING btree (resource_owner_id) WHERE (resource_owner_id IS NOT NULL);


--
-- Name: index_oauth_access_tokens_on_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_oauth_access_tokens_on_token ON public.oauth_access_tokens USING btree (token);


--
-- Name: index_oauth_applications_on_owner_id_and_owner_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_oauth_applications_on_owner_id_and_owner_type ON public.oauth_applications USING btree (owner_id, owner_type);


--
-- Name: index_oauth_applications_on_superapp; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_oauth_applications_on_superapp ON public.oauth_applications USING btree (superapp) WHERE (superapp = true);


--
-- Name: index_oauth_applications_on_uid; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_oauth_applications_on_uid ON public.oauth_applications USING btree (uid);


--
-- Name: index_pghero_space_stats_on_database_and_captured_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_pghero_space_stats_on_database_and_captured_at ON public.pghero_space_stats USING btree (database, captured_at);


--
-- Name: index_poll_votes_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_poll_votes_on_account_id ON public.poll_votes USING btree (account_id);


--
-- Name: index_poll_votes_on_poll_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_poll_votes_on_poll_id ON public.poll_votes USING btree (poll_id);


--
-- Name: index_polls_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_polls_on_account_id ON public.polls USING btree (account_id);


--
-- Name: index_polls_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_polls_on_status_id ON public.polls USING btree (status_id);


--
-- Name: index_preview_card_providers_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_preview_card_providers_on_domain ON public.preview_card_providers USING btree (domain);


--
-- Name: index_preview_card_trends_on_preview_card_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_preview_card_trends_on_preview_card_id ON public.preview_card_trends USING btree (preview_card_id);


--
-- Name: index_preview_cards_on_author_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_preview_cards_on_author_account_id ON public.preview_cards USING btree (author_account_id) WHERE (author_account_id IS NOT NULL);


--
-- Name: index_preview_cards_on_unverified_author_account_id_and_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_preview_cards_on_unverified_author_account_id_and_id ON public.preview_cards USING btree (unverified_author_account_id, id) WHERE (unverified_author_account_id IS NOT NULL);


--
-- Name: index_preview_cards_on_url; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_preview_cards_on_url ON public.preview_cards USING btree (url);


--
-- Name: index_quotes_on_account_id_and_quoted_account_id_and_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_quotes_on_account_id_and_quoted_account_id_and_id ON public.quotes USING btree (account_id, quoted_account_id, id);


--
-- Name: index_quotes_on_activity_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_quotes_on_activity_uri ON public.quotes USING btree (activity_uri) WHERE (activity_uri IS NOT NULL);


--
-- Name: index_quotes_on_approval_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_quotes_on_approval_uri ON public.quotes USING btree (approval_uri) WHERE (approval_uri IS NOT NULL);


--
-- Name: index_quotes_on_quoted_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_quotes_on_quoted_account_id ON public.quotes USING btree (quoted_account_id);


--
-- Name: index_quotes_on_quoted_status_id_and_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_quotes_on_quoted_status_id_and_id ON public.quotes USING btree (quoted_status_id, id);


--
-- Name: index_quotes_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_quotes_on_status_id ON public.quotes USING btree (status_id);


--
-- Name: index_relationship_severance_events_on_type_and_target_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_relationship_severance_events_on_type_and_target_name ON public.relationship_severance_events USING btree (type, target_name);


--
-- Name: index_report_notes_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_report_notes_on_account_id ON public.report_notes USING btree (account_id);


--
-- Name: index_report_notes_on_report_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_report_notes_on_report_id ON public.report_notes USING btree (report_id);


--
-- Name: index_reports_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_reports_on_account_id ON public.reports USING btree (account_id);


--
-- Name: index_reports_on_action_taken_by_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_reports_on_action_taken_by_account_id ON public.reports USING btree (action_taken_by_account_id) WHERE (action_taken_by_account_id IS NOT NULL);


--
-- Name: index_reports_on_assigned_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_reports_on_assigned_account_id ON public.reports USING btree (assigned_account_id) WHERE (assigned_account_id IS NOT NULL);


--
-- Name: index_reports_on_target_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_reports_on_target_account_id ON public.reports USING btree (target_account_id);


--
-- Name: index_rule_translations_on_rule_id_and_language; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_rule_translations_on_rule_id_and_language ON public.rule_translations USING btree (rule_id, language);


--
-- Name: index_scheduled_statuses_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_scheduled_statuses_on_account_id ON public.scheduled_statuses USING btree (account_id);


--
-- Name: index_scheduled_statuses_on_scheduled_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_scheduled_statuses_on_scheduled_at ON public.scheduled_statuses USING btree (scheduled_at);


--
-- Name: index_session_activations_on_access_token_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_session_activations_on_access_token_id ON public.session_activations USING btree (access_token_id);


--
-- Name: index_session_activations_on_session_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_session_activations_on_session_id ON public.session_activations USING btree (session_id);


--
-- Name: index_session_activations_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_session_activations_on_user_id ON public.session_activations USING btree (user_id);


--
-- Name: index_settings_on_var; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_settings_on_var ON public.settings USING btree (var);


--
-- Name: index_severed_relationships_on_local_account_and_event; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_severed_relationships_on_local_account_and_event ON public.severed_relationships USING btree (local_account_id, relationship_severance_event_id);


--
-- Name: index_severed_relationships_on_remote_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_severed_relationships_on_remote_account_id ON public.severed_relationships USING btree (remote_account_id);


--
-- Name: index_severed_relationships_on_unique_tuples; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_severed_relationships_on_unique_tuples ON public.severed_relationships USING btree (relationship_severance_event_id, local_account_id, direction, remote_account_id);


--
-- Name: index_site_uploads_on_var; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_site_uploads_on_var ON public.site_uploads USING btree (var);


--
-- Name: index_software_updates_on_version; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_software_updates_on_version ON public.software_updates USING btree (version);


--
-- Name: index_status_edits_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_status_edits_on_account_id ON public.status_edits USING btree (account_id);


--
-- Name: index_status_edits_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_status_edits_on_status_id ON public.status_edits USING btree (status_id);


--
-- Name: index_status_pins_on_account_id_and_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_status_pins_on_account_id_and_status_id ON public.status_pins USING btree (account_id, status_id);


--
-- Name: index_status_pins_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_status_pins_on_status_id ON public.status_pins USING btree (status_id);


--
-- Name: index_status_stats_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_status_stats_on_status_id ON public.status_stats USING btree (status_id);


--
-- Name: index_status_trends_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_status_trends_on_account_id ON public.status_trends USING btree (account_id);


--
-- Name: index_status_trends_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_status_trends_on_status_id ON public.status_trends USING btree (status_id);


--
-- Name: index_statuses_20190820; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_20190820 ON public.statuses USING btree (account_id, id DESC, visibility, updated_at) WHERE (deleted_at IS NULL);


--
-- Name: index_statuses_local_20190824; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_local_20190824 ON public.statuses USING btree (id DESC, account_id) WHERE ((local OR (uri IS NULL)) AND (deleted_at IS NULL) AND (visibility = 0) AND (reblog_of_id IS NULL) AND ((NOT reply) OR (in_reply_to_account_id = account_id)));


--
-- Name: index_statuses_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_on_account_id ON public.statuses USING btree (account_id);


--
-- Name: index_statuses_on_conversation_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_on_conversation_id ON public.statuses USING btree (conversation_id);


--
-- Name: index_statuses_on_deleted_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_on_deleted_at ON public.statuses USING btree (deleted_at) WHERE (deleted_at IS NOT NULL);


--
-- Name: index_statuses_on_in_reply_to_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_on_in_reply_to_account_id ON public.statuses USING btree (in_reply_to_account_id) WHERE (in_reply_to_account_id IS NOT NULL);


--
-- Name: index_statuses_on_in_reply_to_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_on_in_reply_to_id ON public.statuses USING btree (in_reply_to_id) WHERE (in_reply_to_id IS NOT NULL);


--
-- Name: index_statuses_on_reblog_of_id_and_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_on_reblog_of_id_and_account_id ON public.statuses USING btree (reblog_of_id, account_id);


--
-- Name: index_statuses_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_statuses_on_uri ON public.statuses USING btree (uri text_pattern_ops) WHERE (uri IS NOT NULL);


--
-- Name: index_statuses_public_20250129; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_public_20250129 ON public.statuses USING btree (id DESC, language, account_id) WHERE ((deleted_at IS NULL) AND (visibility = 0) AND (reblog_of_id IS NULL) AND ((NOT reply) OR (in_reply_to_account_id = account_id)));


--
-- Name: index_statuses_tags_on_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_statuses_tags_on_status_id ON public.statuses_tags USING btree (status_id);


--
-- Name: index_tag_follows_on_account_id_and_tag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_tag_follows_on_account_id_and_tag_id ON public.tag_follows USING btree (account_id, tag_id);


--
-- Name: index_tag_follows_on_tag_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_tag_follows_on_tag_id ON public.tag_follows USING btree (tag_id);


--
-- Name: index_tag_trends_on_tag_id_and_language; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_tag_trends_on_tag_id_and_language ON public.tag_trends USING btree (tag_id, language);


--
-- Name: index_tagged_objects_on_object; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_tagged_objects_on_object ON public.tagged_objects USING btree (object_type, object_id);


--
-- Name: index_tagged_objects_on_status_id_and_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_tagged_objects_on_status_id_and_uri ON public.tagged_objects USING btree (status_id, uri) WHERE (uri IS NOT NULL);


--
-- Name: index_tags_on_name_lower_btree; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_tags_on_name_lower_btree ON public.tags USING btree (lower((name)::text) text_pattern_ops);


--
-- Name: index_terms_of_services_on_effective_date; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_terms_of_services_on_effective_date ON public.terms_of_services USING btree (effective_date) WHERE (effective_date IS NOT NULL);


--
-- Name: index_tombstones_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_tombstones_on_account_id ON public.tombstones USING btree (account_id);


--
-- Name: index_tombstones_on_uri; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_tombstones_on_uri ON public.tombstones USING btree (uri);


--
-- Name: index_unavailable_domains_on_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_unavailable_domains_on_domain ON public.unavailable_domains USING btree (domain);


--
-- Name: index_unique_conversations; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_unique_conversations ON public.account_conversations USING btree (account_id, conversation_id, participant_account_ids);


--
-- Name: index_user_invite_requests_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_user_invite_requests_on_user_id ON public.user_invite_requests USING btree (user_id);


--
-- Name: index_username_blocks_on_normalized_username; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_username_blocks_on_normalized_username ON public.username_blocks USING btree (normalized_username);


--
-- Name: index_username_blocks_on_username_lower_btree; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_username_blocks_on_username_lower_btree ON public.username_blocks USING btree (lower((username)::text));


--
-- Name: index_users_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_account_id ON public.users USING btree (account_id);


--
-- Name: index_users_on_confirmation_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_users_on_confirmation_token ON public.users USING btree (confirmation_token);


--
-- Name: index_users_on_created_by_application_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_created_by_application_id ON public.users USING btree (created_by_application_id) WHERE (created_by_application_id IS NOT NULL);


--
-- Name: index_users_on_email; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_users_on_email ON public.users USING btree (email);


--
-- Name: index_users_on_reset_password_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_users_on_reset_password_token ON public.users USING btree (reset_password_token text_pattern_ops) WHERE (reset_password_token IS NOT NULL);


--
-- Name: index_users_on_role_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_role_id ON public.users USING btree (role_id) WHERE (role_id IS NOT NULL);


--
-- Name: index_users_on_unconfirmed_email; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_unconfirmed_email ON public.users USING btree (unconfirmed_email) WHERE (unconfirmed_email IS NOT NULL);


--
-- Name: index_web_push_subscriptions_on_access_token_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_web_push_subscriptions_on_access_token_id ON public.web_push_subscriptions USING btree (access_token_id) WHERE (access_token_id IS NOT NULL);


--
-- Name: index_web_push_subscriptions_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_web_push_subscriptions_on_user_id ON public.web_push_subscriptions USING btree (user_id);


--
-- Name: index_web_settings_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_web_settings_on_user_id ON public.web_settings USING btree (user_id);


--
-- Name: index_webauthn_credentials_on_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_webauthn_credentials_on_external_id ON public.webauthn_credentials USING btree (external_id);


--
-- Name: index_webauthn_credentials_on_user_id_and_nickname; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_webauthn_credentials_on_user_id_and_nickname ON public.webauthn_credentials USING btree (user_id, nickname);


--
-- Name: index_webhooks_on_url; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_webhooks_on_url ON public.webhooks USING btree (url);


--
-- Name: search_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX search_index ON public.accounts USING gin ((((setweight(to_tsvector('simple'::regconfig, (display_name)::text), 'A'::"char") || setweight(to_tsvector('simple'::regconfig, (username)::text), 'B'::"char")) || setweight(to_tsvector('simple'::regconfig, (COALESCE(domain, ''::character varying))::text), 'C'::"char"))));


--
-- Name: web_settings fk_11910667b2; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_settings
    ADD CONSTRAINT fk_11910667b2 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: account_domain_blocks fk_206c6029bd; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_domain_blocks
    ADD CONSTRAINT fk_206c6029bd FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: conversation_mutes fk_225b4212bb; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_mutes
    ADD CONSTRAINT fk_225b4212bb FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: statuses_tags fk_3081861e21; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses_tags
    ADD CONSTRAINT fk_3081861e21 FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE;


--
-- Name: follows fk_32ed1b5560; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follows
    ADD CONSTRAINT fk_32ed1b5560 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: oauth_access_grants fk_34d54b0a33; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_grants
    ADD CONSTRAINT fk_34d54b0a33 FOREIGN KEY (application_id) REFERENCES public.oauth_applications(id) ON DELETE CASCADE;


--
-- Name: blocks fk_4269e03e65; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.blocks
    ADD CONSTRAINT fk_4269e03e65 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: reports fk_4b81f7522c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT fk_4b81f7522c FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: users fk_50500f500d; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_50500f500d FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: favourites fk_5eb6c2b873; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favourites
    ADD CONSTRAINT fk_5eb6c2b873 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: oauth_access_grants fk_63b044929b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_grants
    ADD CONSTRAINT fk_63b044929b FOREIGN KEY (resource_owner_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: follows fk_745ca29eac; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follows
    ADD CONSTRAINT fk_745ca29eac FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: follow_requests fk_76d644b0e7; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_requests
    ADD CONSTRAINT fk_76d644b0e7 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: follow_requests fk_9291ec025d; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_requests
    ADD CONSTRAINT fk_9291ec025d FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: blocks fk_9571bfabc1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.blocks
    ADD CONSTRAINT fk_9571bfabc1 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: session_activations fk_957e5bda89; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.session_activations
    ADD CONSTRAINT fk_957e5bda89 FOREIGN KEY (access_token_id) REFERENCES public.oauth_access_tokens(id) ON DELETE CASCADE;


--
-- Name: media_attachments fk_96dd81e81b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.media_attachments
    ADD CONSTRAINT fk_96dd81e81b FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: mentions fk_970d43f9d1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mentions
    ADD CONSTRAINT fk_970d43f9d1 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: statuses fk_9bda1543f7; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses
    ADD CONSTRAINT fk_9bda1543f7 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: oauth_applications fk_b0988c7c0a; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_applications
    ADD CONSTRAINT fk_b0988c7c0a FOREIGN KEY (owner_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: favourites fk_b0e856845e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.favourites
    ADD CONSTRAINT fk_b0e856845e FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: mutes fk_b8d8daf315; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mutes
    ADD CONSTRAINT fk_b8d8daf315 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: reports fk_bca45b75fd; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT fk_bca45b75fd FOREIGN KEY (action_taken_by_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: identities fk_bea040f377; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identities
    ADD CONSTRAINT fk_bea040f377 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: notifications fk_c141c8ee55; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT fk_c141c8ee55 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: statuses fk_c7fa917661; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses
    ADD CONSTRAINT fk_c7fa917661 FOREIGN KEY (in_reply_to_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: status_pins fk_d4cb435b62; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_pins
    ADD CONSTRAINT fk_d4cb435b62 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: session_activations fk_e5fda67334; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.session_activations
    ADD CONSTRAINT fk_e5fda67334 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: oauth_access_tokens fk_e84df68546; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_tokens
    ADD CONSTRAINT fk_e84df68546 FOREIGN KEY (resource_owner_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: reports fk_eb37af34f0; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT fk_eb37af34f0 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: mutes fk_eecff219ea; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mutes
    ADD CONSTRAINT fk_eecff219ea FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: oauth_access_tokens fk_f5fc4c1ee3; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_access_tokens
    ADD CONSTRAINT fk_f5fc4c1ee3 FOREIGN KEY (application_id) REFERENCES public.oauth_applications(id) ON DELETE CASCADE;


--
-- Name: notifications fk_fbd6b0bf9e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT fk_fbd6b0bf9e FOREIGN KEY (from_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_relationship_severance_events fk_rails_030c916965; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_relationship_severance_events
    ADD CONSTRAINT fk_rails_030c916965 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: collection_reports fk_rails_0720c1a3d6; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_reports
    ADD CONSTRAINT fk_rails_0720c1a3d6 FOREIGN KEY (collection_id) REFERENCES public.collections(id) ON DELETE CASCADE;


--
-- Name: tagged_objects fk_rails_087c1d32f7; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tagged_objects
    ADD CONSTRAINT fk_rails_087c1d32f7 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: tag_follows fk_rails_091e831473; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_follows
    ADD CONSTRAINT fk_rails_091e831473 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: backups fk_rails_096669d221; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.backups
    ADD CONSTRAINT fk_rails_096669d221 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;


--
-- Name: tag_follows fk_rails_0deefe597f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_follows
    ADD CONSTRAINT fk_rails_0deefe597f FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE;


--
-- Name: bookmarks fk_rails_11207ffcfd; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT fk_rails_11207ffcfd FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: account_conversations fk_rails_1491654f9f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_conversations
    ADD CONSTRAINT fk_rails_1491654f9f FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;


--
-- Name: featured_tags fk_rails_174efcf15f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.featured_tags
    ADD CONSTRAINT fk_rails_174efcf15f FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: bulk_imports fk_rails_1d89c0f8b2; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bulk_imports
    ADD CONSTRAINT fk_rails_1d89c0f8b2 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: canonical_email_blocks fk_rails_1ecb262096; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.canonical_email_blocks
    ADD CONSTRAINT fk_rails_1ecb262096 FOREIGN KEY (reference_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_stats fk_rails_215bb31ff1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_stats
    ADD CONSTRAINT fk_rails_215bb31ff1 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: accounts fk_rails_2320833084; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT fk_rails_2320833084 FOREIGN KEY (moved_to_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: featured_tags fk_rails_23a9055c7c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.featured_tags
    ADD CONSTRAINT fk_rails_23a9055c7c FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE;


--
-- Name: scheduled_statuses fk_rails_23bd9018f9; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scheduled_statuses
    ADD CONSTRAINT fk_rails_23bd9018f9 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_statuses_cleanup_policies fk_rails_23d5f73cfe; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_statuses_cleanup_policies
    ADD CONSTRAINT fk_rails_23d5f73cfe FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: statuses fk_rails_256483a9ab; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses
    ADD CONSTRAINT fk_rails_256483a9ab FOREIGN KEY (reblog_of_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: account_notes fk_rails_2801b48f1a; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_notes
    ADD CONSTRAINT fk_rails_2801b48f1a FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: email_subscriptions fk_rails_282940e759; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_subscriptions
    ADD CONSTRAINT fk_rails_282940e759 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: collection_items fk_rails_2eb992658d; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_items
    ADD CONSTRAINT fk_rails_2eb992658d FOREIGN KEY (account_id) REFERENCES public.accounts(id);


--
-- Name: custom_filter_statuses fk_rails_2f6d20c0cf; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_statuses
    ADD CONSTRAINT fk_rails_2f6d20c0cf FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: tag_trends fk_rails_3033046460; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_trends
    ADD CONSTRAINT fk_rails_3033046460 FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE;


--
-- Name: media_attachments fk_rails_31fc5aeef1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.media_attachments
    ADD CONSTRAINT fk_rails_31fc5aeef1 FOREIGN KEY (scheduled_status_id) REFERENCES public.scheduled_statuses(id) ON DELETE SET NULL;


--
-- Name: quotes fk_rails_36d54169fc; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.quotes
    ADD CONSTRAINT fk_rails_36d54169fc FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: preview_card_trends fk_rails_371593db34; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_card_trends
    ADD CONSTRAINT fk_rails_371593db34 FOREIGN KEY (preview_card_id) REFERENCES public.preview_cards(id) ON DELETE CASCADE;


--
-- Name: user_invite_requests fk_rails_3773f15361; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_invite_requests
    ADD CONSTRAINT fk_rails_3773f15361 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: quotes fk_rails_38068caa0e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.quotes
    ADD CONSTRAINT fk_rails_38068caa0e FOREIGN KEY (quoted_status_id) REFERENCES public.statuses(id) ON DELETE SET NULL;


--
-- Name: lists fk_rails_3853b78dac; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.lists
    ADD CONSTRAINT fk_rails_3853b78dac FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: reports fk_rails_3deb8c7acb; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT fk_rails_3deb8c7acb FOREIGN KEY (application_id) REFERENCES public.oauth_applications(id) ON DELETE SET NULL;


--
-- Name: polls fk_rails_3e0d9f1115; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.polls
    ADD CONSTRAINT fk_rails_3e0d9f1115 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: media_attachments fk_rails_3ec0cfdd70; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.media_attachments
    ADD CONSTRAINT fk_rails_3ec0cfdd70 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE SET NULL;


--
-- Name: account_moderation_notes fk_rails_3f8b75089b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_moderation_notes
    ADD CONSTRAINT fk_rails_3f8b75089b FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: email_domain_blocks fk_rails_408efe0a15; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_domain_blocks
    ADD CONSTRAINT fk_rails_408efe0a15 FOREIGN KEY (parent_id) REFERENCES public.email_domain_blocks(id) ON DELETE CASCADE;


--
-- Name: list_accounts fk_rails_40f9cc29f1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.list_accounts
    ADD CONSTRAINT fk_rails_40f9cc29f1 FOREIGN KEY (follow_id) REFERENCES public.follows(id) ON DELETE CASCADE;


--
-- Name: account_deletion_requests fk_rails_45bf2626b9; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_deletion_requests
    ADD CONSTRAINT fk_rails_45bf2626b9 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: status_stats fk_rails_4a247aac42; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_stats
    ADD CONSTRAINT fk_rails_4a247aac42 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: collection_reports fk_rails_4a504bd5e6; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_reports
    ADD CONSTRAINT fk_rails_4a504bd5e6 FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: fasp_subscriptions fk_rails_4c021f5938; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_subscriptions
    ADD CONSTRAINT fk_rails_4c021f5938 FOREIGN KEY (fasp_provider_id) REFERENCES public.fasp_providers(id);


--
-- Name: generated_annual_reports fk_rails_4ca37f035c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.generated_annual_reports
    ADD CONSTRAINT fk_rails_4ca37f035c FOREIGN KEY (account_id) REFERENCES public.accounts(id);


--
-- Name: reports fk_rails_4e7a498fb4; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT fk_rails_4e7a498fb4 FOREIGN KEY (assigned_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: account_notes fk_rails_4ee4503c69; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_notes
    ADD CONSTRAINT fk_rails_4ee4503c69 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: appeals fk_rails_501c3a6e13; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appeals
    ADD CONSTRAINT fk_rails_501c3a6e13 FOREIGN KEY (rejected_by_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: severed_relationships fk_rails_5054494e1e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.severed_relationships
    ADD CONSTRAINT fk_rails_5054494e1e FOREIGN KEY (relationship_severance_event_id) REFERENCES public.relationship_severance_events(id) ON DELETE CASCADE;


--
-- Name: notification_policies fk_rails_506d62f0da; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_policies
    ADD CONSTRAINT fk_rails_506d62f0da FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: collections fk_rails_544f142936; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collections
    ADD CONSTRAINT fk_rails_544f142936 FOREIGN KEY (account_id) REFERENCES public.accounts(id);


--
-- Name: notification_requests fk_rails_5632f121b4; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_requests
    ADD CONSTRAINT fk_rails_5632f121b4 FOREIGN KEY (from_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: mentions fk_rails_59edbe2887; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mentions
    ADD CONSTRAINT fk_rails_59edbe2887 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: custom_filter_keywords fk_rails_5a49a74012; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_keywords
    ADD CONSTRAINT fk_rails_5a49a74012 FOREIGN KEY (custom_filter_id) REFERENCES public.custom_filters(id) ON DELETE CASCADE;


--
-- Name: conversation_mutes fk_rails_5ab139311f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_mutes
    ADD CONSTRAINT fk_rails_5ab139311f FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;


--
-- Name: polls fk_rails_5b19a0c011; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.polls
    ADD CONSTRAINT fk_rails_5b19a0c011 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: fasp_follow_recommendations fk_rails_5c63a5fd1b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_follow_recommendations
    ADD CONSTRAINT fk_rails_5c63a5fd1b FOREIGN KEY (recommended_account_id) REFERENCES public.accounts(id);


--
-- Name: notification_requests fk_rails_61c7aa9c1f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_requests
    ADD CONSTRAINT fk_rails_61c7aa9c1f FOREIGN KEY (last_status_id) REFERENCES public.statuses(id) ON DELETE SET NULL;


--
-- Name: instance_moderation_notes fk_rails_62f919e09b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.instance_moderation_notes
    ADD CONSTRAINT fk_rails_62f919e09b FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: users fk_rails_642f17018b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_rails_642f17018b FOREIGN KEY (role_id) REFERENCES public.user_roles(id) ON DELETE SET NULL;


--
-- Name: status_pins fk_rails_65c05552f1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_pins
    ADD CONSTRAINT fk_rails_65c05552f1 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: status_trends fk_rails_68c610dc1a; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_trends
    ADD CONSTRAINT fk_rails_68c610dc1a FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: account_conversations fk_rails_6f5278b6e9; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_conversations
    ADD CONSTRAINT fk_rails_6f5278b6e9 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: preview_cards fk_rails_6fb2119894; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_cards
    ADD CONSTRAINT fk_rails_6fb2119894 FOREIGN KEY (unverified_author_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: collections fk_rails_70f13aad15; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collections
    ADD CONSTRAINT fk_rails_70f13aad15 FOREIGN KEY (tag_id) REFERENCES public.tags(id);


--
-- Name: fasp_follow_recommendations fk_rails_71623d7e2c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_follow_recommendations
    ADD CONSTRAINT fk_rails_71623d7e2c FOREIGN KEY (requesting_account_id) REFERENCES public.accounts(id);


--
-- Name: announcement_reactions fk_rails_7444ad831f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_reactions
    ADD CONSTRAINT fk_rails_7444ad831f FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: web_push_subscriptions fk_rails_751a9f390b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_push_subscriptions
    ADD CONSTRAINT fk_rails_751a9f390b FOREIGN KEY (access_token_id) REFERENCES public.oauth_access_tokens(id) ON DELETE CASCADE;


--
-- Name: fasp_backfill_requests fk_rails_760d761775; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_backfill_requests
    ADD CONSTRAINT fk_rails_760d761775 FOREIGN KEY (fasp_provider_id) REFERENCES public.fasp_providers(id);


--
-- Name: notification_permissions fk_rails_7c0bed08df; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_permissions
    ADD CONSTRAINT fk_rails_7c0bed08df FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: report_notes fk_rails_7fa83a61eb; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_notes
    ADD CONSTRAINT fk_rails_7fa83a61eb FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: list_accounts fk_rails_85fee9d6ab; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.list_accounts
    ADD CONSTRAINT fk_rails_85fee9d6ab FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: notification_requests fk_rails_881c7f71c4; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_requests
    ADD CONSTRAINT fk_rails_881c7f71c4 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_relationship_severance_events fk_rails_8a34c3a361; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_relationship_severance_events
    ADD CONSTRAINT fk_rails_8a34c3a361 FOREIGN KEY (relationship_severance_event_id) REFERENCES public.relationship_severance_events(id) ON DELETE CASCADE;


--
-- Name: custom_filters fk_rails_8b8d786993; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filters
    ADD CONSTRAINT fk_rails_8b8d786993 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_warnings fk_rails_8f2bab4b16; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warnings
    ADD CONSTRAINT fk_rails_8f2bab4b16 FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: users fk_rails_8fb2a43e88; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_rails_8fb2a43e88 FOREIGN KEY (invite_id) REFERENCES public.invites(id) ON DELETE SET NULL;


--
-- Name: statuses fk_rails_94a6f70399; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses
    ADD CONSTRAINT fk_rails_94a6f70399 FOREIGN KEY (in_reply_to_id) REFERENCES public.statuses(id) ON DELETE SET NULL;


--
-- Name: severed_relationships fk_rails_98ff099d4c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.severed_relationships
    ADD CONSTRAINT fk_rails_98ff099d4c FOREIGN KEY (local_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: announcement_mutes fk_rails_9c99f8e835; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_mutes
    ADD CONSTRAINT fk_rails_9c99f8e835 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: appeals fk_rails_9deb2f63ad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appeals
    ADD CONSTRAINT fk_rails_9deb2f63ad FOREIGN KEY (approved_by_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: bookmarks fk_rails_9f6ac182a6; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT fk_rails_9f6ac182a6 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: announcement_reactions fk_rails_a1226eaa5c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_reactions
    ADD CONSTRAINT fk_rails_a1226eaa5c FOREIGN KEY (announcement_id) REFERENCES public.announcements(id) ON DELETE CASCADE;


--
-- Name: account_pins fk_rails_a176e26c37; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_pins
    ADD CONSTRAINT fk_rails_a176e26c37 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: webauthn_credentials fk_rails_a4355aef77; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT fk_rails_a4355aef77 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: account_warnings fk_rails_a65a1bf71b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warnings
    ADD CONSTRAINT fk_rails_a65a1bf71b FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: status_trends fk_rails_a6b527ea49; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_trends
    ADD CONSTRAINT fk_rails_a6b527ea49 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: poll_votes fk_rails_a6e6974b7e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT fk_rails_a6e6974b7e FOREIGN KEY (poll_id) REFERENCES public.polls(id) ON DELETE CASCADE;


--
-- Name: markers fk_rails_a7009bc2b6; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.markers
    ADD CONSTRAINT fk_rails_a7009bc2b6 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: admin_action_logs fk_rails_a7667297fa; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_action_logs
    ADD CONSTRAINT fk_rails_a7667297fa FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_warnings fk_rails_a7ebbb1e37; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_warnings
    ADD CONSTRAINT fk_rails_a7ebbb1e37 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: status_edits fk_rails_a960f234a0; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_edits
    ADD CONSTRAINT fk_rails_a960f234a0 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: appeals fk_rails_a99f14546e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appeals
    ADD CONSTRAINT fk_rails_a99f14546e FOREIGN KEY (account_warning_id) REFERENCES public.account_warnings(id) ON DELETE CASCADE;


--
-- Name: follow_recommendation_mutes fk_rails_a9f09ec9a8; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_mutes
    ADD CONSTRAINT fk_rails_a9f09ec9a8 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: custom_emoji_categories fk_rails_ad7840c8cf; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emoji_categories
    ADD CONSTRAINT fk_rails_ad7840c8cf FOREIGN KEY (featured_emoji_id) REFERENCES public.custom_emojis(id) ON DELETE SET NULL;


--
-- Name: web_push_subscriptions fk_rails_b006f28dac; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.web_push_subscriptions
    ADD CONSTRAINT fk_rails_b006f28dac FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: collection_items fk_rails_b1a778644b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.collection_items
    ADD CONSTRAINT fk_rails_b1a778644b FOREIGN KEY (collection_id) REFERENCES public.collections(id) ON DELETE CASCADE;


--
-- Name: poll_votes fk_rails_b6c18cf44a; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT fk_rails_b6c18cf44a FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: announcement_reactions fk_rails_b742c91c0e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_reactions
    ADD CONSTRAINT fk_rails_b742c91c0e FOREIGN KEY (custom_emoji_id) REFERENCES public.custom_emojis(id) ON DELETE CASCADE;


--
-- Name: quotes fk_rails_bd3ab4462c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.quotes
    ADD CONSTRAINT fk_rails_bd3ab4462c FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: quotes fk_rails_bfc5276b70; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.quotes
    ADD CONSTRAINT fk_rails_bfc5276b70 FOREIGN KEY (quoted_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: fasp_debug_callbacks fk_rails_c1650087cd; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fasp_debug_callbacks
    ADD CONSTRAINT fk_rails_c1650087cd FOREIGN KEY (fasp_provider_id) REFERENCES public.fasp_providers(id);


--
-- Name: account_migrations fk_rails_c9f701caaf; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_migrations
    ADD CONSTRAINT fk_rails_c9f701caaf FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: report_notes fk_rails_cae66353f3; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_notes
    ADD CONSTRAINT fk_rails_cae66353f3 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: follow_recommendation_mutes fk_rails_d36abd69ea; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_mutes
    ADD CONSTRAINT fk_rails_d36abd69ea FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: bulk_import_rows fk_rails_d39af34335; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bulk_import_rows
    ADD CONSTRAINT fk_rails_d39af34335 FOREIGN KEY (bulk_import_id) REFERENCES public.bulk_imports(id) ON DELETE CASCADE;


--
-- Name: account_pins fk_rails_d44979e5dd; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_pins
    ADD CONSTRAINT fk_rails_d44979e5dd FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: rule_translations fk_rails_d5fd439dde; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rule_translations
    ADD CONSTRAINT fk_rails_d5fd439dde FOREIGN KEY (rule_id) REFERENCES public.rules(id) ON DELETE CASCADE;


--
-- Name: account_migrations fk_rails_d9a8dad070; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_migrations
    ADD CONSTRAINT fk_rails_d9a8dad070 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: status_edits fk_rails_dc8988c545; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.status_edits
    ADD CONSTRAINT fk_rails_dc8988c545 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: preview_cards fk_rails_dca4905b94; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.preview_cards
    ADD CONSTRAINT fk_rails_dca4905b94 FOREIGN KEY (author_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;


--
-- Name: account_moderation_notes fk_rails_dd62ed5ac3; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_moderation_notes
    ADD CONSTRAINT fk_rails_dd62ed5ac3 FOREIGN KEY (target_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: statuses_tags fk_rails_df0fe11427; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.statuses_tags
    ADD CONSTRAINT fk_rails_df0fe11427 FOREIGN KEY (status_id) REFERENCES public.statuses(id) ON DELETE CASCADE;


--
-- Name: follow_recommendation_suppressions fk_rails_dfb9a1dbe2; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follow_recommendation_suppressions
    ADD CONSTRAINT fk_rails_dfb9a1dbe2 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: custom_filter_statuses fk_rails_e2ddaf5b14; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_filter_statuses
    ADD CONSTRAINT fk_rails_e2ddaf5b14 FOREIGN KEY (custom_filter_id) REFERENCES public.custom_filters(id) ON DELETE CASCADE;


--
-- Name: announcement_mutes fk_rails_e35401adf1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcement_mutes
    ADD CONSTRAINT fk_rails_e35401adf1 FOREIGN KEY (announcement_id) REFERENCES public.announcements(id) ON DELETE CASCADE;


--
-- Name: notification_permissions fk_rails_e3e0aaad70; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_permissions
    ADD CONSTRAINT fk_rails_e3e0aaad70 FOREIGN KEY (from_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: login_activities fk_rails_e4b6396b41; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.login_activities
    ADD CONSTRAINT fk_rails_e4b6396b41 FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: list_accounts fk_rails_e54e356c88; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.list_accounts
    ADD CONSTRAINT fk_rails_e54e356c88 FOREIGN KEY (list_id) REFERENCES public.lists(id) ON DELETE CASCADE;


--
-- Name: appeals fk_rails_ea84881569; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appeals
    ADD CONSTRAINT fk_rails_ea84881569 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: users fk_rails_ecc9536e7c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_rails_ecc9536e7c FOREIGN KEY (created_by_application_id) REFERENCES public.oauth_applications(id) ON DELETE SET NULL;


--
-- Name: list_accounts fk_rails_f11f9d1fcc; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.list_accounts
    ADD CONSTRAINT fk_rails_f11f9d1fcc FOREIGN KEY (follow_request_id) REFERENCES public.follow_requests(id) ON DELETE CASCADE;


--
-- Name: keypairs fk_rails_f5ea7ac36a; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.keypairs
    ADD CONSTRAINT fk_rails_f5ea7ac36a FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: severed_relationships fk_rails_f7afd97ba4; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.severed_relationships
    ADD CONSTRAINT fk_rails_f7afd97ba4 FOREIGN KEY (remote_account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: tombstones fk_rails_f95b861449; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tombstones
    ADD CONSTRAINT fk_rails_f95b861449 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: account_aliases fk_rails_fc91575d08; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_aliases
    ADD CONSTRAINT fk_rails_fc91575d08 FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;


--
-- Name: invites fk_rails_ff69dbb2ac; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT fk_rails_ff69dbb2ac FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: account_summaries; Type: MATERIALIZED VIEW DATA; Schema: public; Owner: -
--

REFRESH MATERIALIZED VIEW public.account_summaries;


--
-- Name: global_follow_recommendations; Type: MATERIALIZED VIEW DATA; Schema: public; Owner: -
--

REFRESH MATERIALIZED VIEW public.global_follow_recommendations;


--
-- Name: instances; Type: MATERIALIZED VIEW DATA; Schema: public; Owner: -
--

REFRESH MATERIALIZED VIEW public.instances;


--
-- PostgreSQL database dump complete
--

\unrestrict rustodonMastodon465FixtureDump
