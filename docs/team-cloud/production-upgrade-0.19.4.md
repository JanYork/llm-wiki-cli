# Team service upgrade to 0.19.4

Date: 2026-10-07 (Asia/Shanghai). Scope: the existing production team backend behind HTTPS port 8788. The user explicitly requested this upgrade after publication. Normal local/Pro CLI installations were not changed.

## Running artifact

The service now returns `lwc 0.19.4` from its running container. Its local image is `lwc-team:0.19.4`, ID `sha256:e004a08a74b6f67531105778d1961f2053996ecadb39770931a9c77ce51fe00a`.

GHCR layer downloads were progressing too slowly and were cancelled before downtime. The replacement image uses the retained runtime base (`sha256:6676ba10248f4b7a6267149459a5ea85690bc1e44b5fddc043ed462a14fa4440`) and the verified public 0.19.4 Linux binary and compiled administration assets. It is a local assembly of official release assets, distinct from the published GHCR image. No Rust compilation was performed.

- Release source: `99685aa37f55a4400f5a5aaa94e16ca363229e24`.
- Linux archive SHA-256: `3f1331b588d70e97fd4157220304ecd2065aa7cff0cfffb3ba241b191d3433b4`.
- Team asset archive SHA-256: `ae10660930af6f53b7a4d778237db34b509a13734db93012a478e01c503559c0`.
- Running program artifact SHA-256: `36997924695f5c9bcc4bf22fccd52ffd0312253898bc13efe0a4d9813ef13132`.

The existing persistent memory volume, server configuration, user identity and HTTPS proxy were retained. Only the backend service was recreated; Caddy and other workloads were not restarted.

## Backup and continuity

The old service was stopped before invoking the original program's `server backup`. The complete private backup, original configuration and retained old image are available on the deployment host. Backup directory: `/home/zd/lwc-team/upgrades/20261007-0.19.4-122603/data-backups/snapshot`; its completion marker was verified. Backup took 20.540 seconds; stop-to-verified-HTTPS-health was 29.569 seconds.

Before/after authenticated bounded inventory reads matched all three original spaces: accepted head, epoch, digest, snapshot identity, caller role and aggregate object hashes. All 9,074 original core objects were retained. The comparison was repeated after the synthetic sync smoke. Domain content and credentials were not copied into this report.

TLS was verified with the existing Caddy CA; certificate verification was not disabled. The existing owner's key login, team/project/collection associations, HTML and referenced assets passed the existing deployment probe. The container was running with zero restarts, and unauthenticated API access remained rejected.

## Automatic sync smoke

One explicitly named private synthetic acceptance space was used. Two 0.19.4 CLI containers each had 0.5 CPU and 256 MiB, an isolated private home, and the existing CA. Automatic polling was configured to 1,000 ms. Without manual sync commands, A→cloud→B and B→cloud→A both reproduced the exact page bodies. Cloud-only authenticated reads matched the synchronized page. The space reached accepted head 2.

Measured propagation samples, including the source command, were 20.160 and 40.253 seconds. These are two small-space samples under client resource limits, not a latency percentile or a guarantee of second-level propagation. No production pressure/fault matrix was repeated; broader correctness and bounded-load evidence remains in [the isolated acceptance report](sync-availability-acceptance.md).

Initial harness attempts misused the delegated-credential override during ordinary login and mishandled host ownership of container-created directories. These were corrected in the harness; the final smoke passed. They are not reported as product failures or passing attempts.

Both temporary client containers were removed, automatic sync disabled, sessions revoked, and their credential directories deleted. The clearly named private synthetic acceptance space remains as audit evidence; original business spaces were not used for synthetic writes.

## Recovery boundary

The stopped original-image backup and pre-upgrade Compose configuration remain private on the host. No data restoration was performed. If data recovery is needed, stop the service and use the supported `server restore` command with the current authorization directory and a new destination. Do not overwrite the live volume or replace current permissions with an older backup. See [the deployment recovery procedure](../../deploy/team/README.md).
