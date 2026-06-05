# Triage Labels

The skills speak in terms of five canonical triage roles. With a local-markdown tracker, the chosen string is written to the `Status:` line at the top of each issue file.

| Canonical role    | String for this repo | Meaning                                  |
| ----------------- | -------------------- | ---------------------------------------- |
| `needs-triage`    | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`      | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent` | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human` | `ready-for-human`    | Requires human implementation            |
| `wontfix`         | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), write the corresponding string from this table to the issue's `Status:` line.

Edit the right-hand column to match whatever vocabulary you actually use.
