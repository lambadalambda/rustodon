# Mastodon frontend bundle

- Release: `v4.6.5`
- Revision: `1440d55b139e39ec722c2a3db7f60b66cd889048`
- Node: `>=22`
- Yarn: `4.16.0`
- Build: `yarn install --immutable && yarn build:production`
- Vite manifests: `.vite/manifest.json`, `.vite/manifest-assets.json`

The checked-in `public` artifact is the production output of the pinned
checkout at `target/mastodon-v4.6.5`, including un-hashed files referenced by
the service worker. Verify it after copying it into a deployment artifact with:

```console
sha256sum -c public/packs/SHA256SUMS
```
