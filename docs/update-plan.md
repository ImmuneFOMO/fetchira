# Update lifecycle implementation plan

This is the implementation checklist for the next release. It does not describe features as available until their checks pass.

## User behavior

- `fetchira update` and **Update after active requests** download and verify the release, stop admitting new work, finish active Fetchira requests, and retire affected processes. They do not wait for the coding applications themselves to exit.
- `fetchira update --force` and **Force update now** permit interruption. The UI confirms this once. The updater makes a bounded attempt to deliver an update/reconnect error before retiring affected processes.
- Both modes use English product text, preserve integration choices, and identify coding tools that still need reconnection. Neither promises automatic restart by every MCP client.
- Already-running releases without the control protocol cannot report active work or deliver new protocol errors. Graceful updates explain this limitation; force can retire only positively identified Fetchira processes. The verified installer provides the initial bridge into the new updater.

## Transaction and recovery

1. Serialize updates, resolve the release, verify checksum and executable startup before interrupting work.
2. Identify processes using canonical installation/data paths and process start identity. Never terminate arbitrary processes by basename or stale PID alone.
3. Close admission, drain or cancel active work, stop affected processes, and hold the startup/migration barrier. Include CLI, MCP, dashboard mutations and background writers.
4. Save a consistent database/config snapshot and previous executable; install atomically, migrate under exclusive ownership, and verify integrity/readiness before admitting work.
5. Persist progress and failure details through restart. Recover paired executable/data only before admitting new-version writes; never silently restore an old snapshot over newer user work.

## Hosted

Stage downloads while serving. Maintenance rejects new work and admin mutations, drains or cancels in-flight calls and background writers, and closes database connections before transition. Preserve the data volume, configuration, master key and external secret mounts. Restart the verified runtime, confirm health/schema, and retain a durable result. An up-to-date check must not restart the service.

Application updates replace the Fetchira runtime. Docker image updates also update the OS/browser dependencies and remain a separate operator action using pinned images. Both paths must preserve a newer self-updated runtime unless an explicit compatible rollback was requested.

## Acceptance checks

- Graceful drain waits for an active call, rejects a late call, and completes with idle MCP processes still connected.
- Force interrupts a hung call within its bound; old clients may see transport closure.
- Corrupt downloads, failed migrations, interrupted transitions and concurrent updates preserve recoverable data and never report success prematurely.
- PID reuse, a second home, a second executable, malformed registry entries and unrelated applications do not result in unintended termination.
- macOS/Linux CLI and UI, hosted auth/maintenance/restart, schema/config/session/key preservation, and integration refresh run entirely in isolated test installations.
