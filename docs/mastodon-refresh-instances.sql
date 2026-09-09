-- Run this script as the owner of public.instances in the Mastodon database.
-- Grant EXECUTE to the dedicated Rustodon writer only after this script succeeds.
CREATE OR REPLACE FUNCTION public.rustodon_refresh_instances()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
  REFRESH MATERIALIZED VIEW CONCURRENTLY public.instances;
END
$$;

REVOKE ALL ON FUNCTION public.rustodon_refresh_instances() FROM PUBLIC;
