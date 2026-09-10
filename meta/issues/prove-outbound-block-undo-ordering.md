# Prove outbound Block/Undo ordering

## Summary

Prove that a remote Block delivery remains ahead of its transactional Undo
successor when the first signed POST is still live and multiple Push workers
are competing for the ordered stream.

## Requirements

- Exercise the real restored Mastodon fixture and signed outbound transport.
- Hold the Block request open while recording the matching Undo successor.
- Prove another Push worker cannot claim the successor before the Block is
  acknowledged or abandoned.
- Verify the remote wire observes Block before Undo and that cleanup restores
  the fixture baseline.

## Acceptance Criteria

- `mise run worker-integration` passes the restored Block/Undo ordering case.
- The outbound-delivery issue and acceptance matrix name the completed local
  proof while keeping live peer convergence as a separate external gap.

## Progress

- Added `activitypub_relationship_delivery_keeps_undo_after_live_block` in
  `tests/workers.rs`. The restored fixture holds a signed Block POST open,
  records the Undo successor, proves a second Push worker cannot claim it,
  releases the Block, and verifies Block-before-Undo wire order.
- `mise run worker-integration` passes 45/45, including the new case.
