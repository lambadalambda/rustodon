# Isolated Mastodon peer smoke

The peer runner exercises an actual pinned Mastodon 4.6.5 process and Rustodon
in one disposable, cache-only fixture. It is manual acceptance evidence, not a
normal unit/CI lane or a production test.

Provision the checkout explicitly before running scenarios. The ignored marker
binds approval to the physical repository root; remove it to revoke approval.

```console
mkdir -p target
(umask 077; printf 'rustodon-peer-workspace-v1:%s\n' "$(pwd -P)" \
  > target/.rustodon-peer-workspace)
mise run peer-public
mise run peer-privacy
mise run peer-notes
mise run peer-profile
mise run peer-interactions
mise run peer-replies
```

Each invocation requires the mode-600 marker and starts from the physical
repository root. It confines Cargo output to the repository target, uses a
unique run marker, validates the committed fixture and all immutable image
digests/platforms, and refuses image pulls/builds. A workspace lock prevents
concurrent peer runs. Missing or uncertain prerequisites fail closed.

## Evidence boundary

All five bounded scenarios have passed against the pinned Mastodon peer:

| Scenario | Contract |
| --- | --- |
| `public` | Public discovery, follow, delivery, received-state convergence, and signed Create in both directions. |
| `privacy` | Followers-only/direct delivery plus recipient, nonrecipient, outsider, and anonymous access controls. |
| `notes` | Public/private Note create, update, delete, identity preservation, and visibility preservation. |
| `profile` | Profile text, flags, fields, and avatar/header URL updates through signed actor Update. |
| `interactions` | Like/Undo and followers-only Announce/Undo with exact activity identity and audience. |

The `replies` scenario (added 2026-09-23) has not passed yet: each side replies
to the other's public root, and the author's peer must thread the reply under
its own root and list it in `/context`. Its first NAS attempts hit the runner's
600-second deadline during fixture restore under unrelated disk contention.

The retained run predates later browser/API changes. It does not prove the exact
current tree, an explicit reply flow, simultaneous reciprocal-follow stress,
complete notification/counter parity, successful remote profile-media download,
all federation implementations, production behavior, or Pleroma.

## Isolation model

The fixture uses:

- committed Mastodon 4.6.5 schema/media metadata;
- cached immutable Mastodon, PostgreSQL, and Redis images;
- two independently cloned and emptied databases;
- fresh functional local users, OAuth tokens, and signing keypairs on each side;
- separate media roots and `.invalid` origins;
- Mastodon Puma and Sidekiq plus Rustodon web and durable workers;
- separate least-privilege runtime and writer roles;
- two loopback TLS relays with an ephemeral CA.

No remote actors, follows, statuses, or signing keys are copied between peers.
Original HTTPS URLs, Host values, signatures, and TLS server-name verification
remain intact. The relays forward only to fixed loopback backends and bound
request bodies and timeouts.

Before mutation the runner verifies that its intended containers, network, and
volume do not already exist. Podman is forced into local mode with a minimal
environment, and a mount/network probe confirms that the engine sees the same
absolute run directory and host network namespace. It rejects
production-looking resources and any selected test that is absent or no longer
ignored.

## Test-only routing

Rustodon's exact origin map and CA are enabled together through
`RUSTODON_TEST_PEER_ORIGINS` and `RUSTODON_TEST_PEER_CA`, only in debug builds
with `test-support`. Every unmapped destination fails closed. This is not a
release SSRF exemption, DNS/public-IP spoof, proxy tunnel, or TLS-verification
bypass.

A disposable Mastodon initializer maps the same exact `.invalid` HTTPS origins
to loopback while retaining HTTP.rb TLS, signing, and response handling. It does
not modify upstream source or install a private-address exception in production
configuration.

## Scenario assertions

### Discovery, follow, and public delivery

The runner starts with no cached remote accounts. Each side resolves the other
through `/api/v1/accounts/search?resolve=true`, verifies actor URI and signing
key, and follows in sequence. It waits for sender/receiver rows and absence of
pending requests before reversing direction.

Each side then creates a public status. Success requires the receiving database
to contain the exact object URI, actor URI, visibility, remote flag, and marker,
plus a successful signed inbox Create with matching actor/object/public
audience. The test rejects canonical status URL fetches; a queued HTTP 2xx alone
is not convergence.

### Private and direct delivery

`privacy` creates followers-only and direct Notes in both directions. A normal
peer follows the author, a separate non-following account receives direct Notes,
and a third account acts as outsider.

Assertions bind received identity, visibility, marker, signed Create, and exact
audiences. Public-addressed attempts for private objects fail the scenario even
when a correct private delivery also succeeds. Recipient access must succeed;
nonrecipient, outsider, and anonymous REST access must fail on both sides.

### Note lifecycle

`notes` creates public and followers-only Notes in both directions, observes
received state and signed Create, edits content and warning, and requires Update
to modify the same row without changing URI or visibility. Private access remains
restricted after editing. Delete requires signed Delete, retirement of the
observed row, and subsequent REST absence.

The test continually rejects canonical status GETs as a substitute for push
processing. Tombstone audience privacy is not asserted.

### Profile updates

After reciprocal follows, each author updates text, account flags, a profile
field, and avatar/header uploads in one request. Receiver assertions bind the
existing actor identity, rendered note, flags, field, exact advertised media
URLs, and signed inbox Update. Actor URL GETs after mutation are rejected so a
refresh cannot substitute for Update ingestion.

URL convergence does not prove that the receiving peer downloaded profile media;
that behavior has separate fixture coverage.

### Interactions and Undo

Each direction receives a new public Note, likes and unlikes it, and verifies the
favourite row plus a signed Undo whose object is the exact observed Like ID.

It then creates a followers-only boost of the public Note. Assertions require the
exact Announce ID, actor, target, followers audience, successful signed delivery,
private wrapper visibility, and outsider/anonymous denial. Undo retires the
wrapper while leaving the original public Note active.

Announce-envelope audiences are tracked separately from the embedded Note.
Failed or in-flight forwarding cannot count as success, and only completed 2xx
delivery events satisfy the positive wire assertion.

## Bounds and cleanup

Scenarios have bounded overall deadlines, SQL statements, pool acquisition,
application processes, and container resources. Execution is sequential rather
than simultaneous convergence stress.

Cleanup terminates task-owned processes, captures bounded diagnostic metadata,
removes task-owned containers/volume/network, verifies absence, and removes
generated keys, certificates, and token-bearing environment files. Logs remain
under the ignored run directory. Before creating resources, the runner records
their exact kinds and names in `.peer-resources` beside the run marker. A forced
process kill cannot execute cleanup; in that case use this inventory to inspect
and remove only those exact resources, never use a broad prune.

See [Testing](testing.md#federation-peer-testing) for gate classification and
portable prerequisite rules.
