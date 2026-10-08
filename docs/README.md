# Documentation

Back to the [project README](../README.md).

| Document | What it is |
| --- | --- |
| **Using it** | |
| [reference/keys.md](reference/keys.md) | Every key and mouse action (the app shows the same list with `H` or `?`) |
| **Planning** | |
| [planning/PLAN.md](planning/PLAN.md) | Architecture, the milestones and what each release round added |
| [planning/v1.1-media-server.md](planning/v1.1-media-server.md) | The 1.1 plan: the media server as drop-ins, with estimates and risks |
| [planning/streaming-services.md](planning/streaming-services.md) | Spotify, Apple Music, YouTube Music, Amazon Music: what is possible and how |
| **Reference** | |
| [reference/development.md](reference/development.md) | Crate layout, build and test commands |
| [reference/host-api.md](reference/host-api.md) | The host-neutral interfaces (now-playing, library, visualizer, platform decoders, quitting) |
| [reference/updates.md](reference/updates.md) | How the in-app updater works and what it trusts |
| **Releasing** | |
| [release/packaging.md](release/packaging.md) | Building and signing every package, the release site, version spellings |
| [release/release-testing.md](release/release-testing.md) | The manual test checklist for every package and OS |
| **History** | |
| [audits/](audits/audit-1.0.0-rc1.md) | The 1.0.0-rc1 audit and its fixes |
| [reviews/](reviews/app-api-v0-review.md) | The Rusty Bucket App API review |
| [screenshots/](screenshots/) | Screenshots of the current UI (`tools/screenshots-desktop.sh`, `cargo xtask e2e --screenshots`) |

`python3 tools/check-links.py` checks that every link in these documents works.
