# Recorded examples

These recordings show Beam's terminal output. Use an asciinema-compatible player to open the `.cast` files.

| Recording | What it shows |
|---|---|
| [Send animation](send.cast) | A voxel workspace disperses, moves to the sandbox, and forms again. |
| [Return animation](return.cast) | The same timeline runs in reverse to bring the workspace home. |
| [Workflow examples](workflows.cast) | A transfer plan, saved-return review, apply, undo, and conflict recovery. |

The animation recordings run `beam demo --seconds 6`, with `--down` for return. The demo transfers no files. Press **Esc** or **q** to stop playback in Beam; press **Ctrl-C** to interrupt it.

The workflow recording runs real Beam commands against temporary local Git snapshots. It begins with a plan that stops at a missing-credential check. Return examples start from fixture packages that are already downloaded. The script checks that apply brings files back, undo restores local state, and conflicting edits remain local.

There is no real sandbox in the workflow fixture. The `--keep` examples skip provider cleanup. The script closes fixture receipts outside the recorded commands. Paths use `/tmp/beam-example` in the recording. These examples do not verify provider transfer, cleanup, or authenticated agent continuation.

To regenerate the recordings:

```sh
python3 scripts/test_demo.py --record docs/demos
python3 scripts/record_workflows.py
```

The scripts use Python's standard library and the existing Rust build. An asciinema recorder is not required.

## Display controls

| Setting | Behavior |
|---|---|
| `BEAM_ANIMATION=0` | Disable all motion. Use static progress and a static demo. |
| `BEAM_EFFECT=off` | Disable ambient graphics and shader activation. Keep the inline spinner. |
| `BEAM_EFFECT=graphics` | Use the image effect on supported terminals. |
| `BEAM_EFFECT=shader` | Signal the optional shader in Ghostty. Requires manual shader installation. |
| `NO_COLOR=1` | Use plain text without animated display control sequences. |

Essential text follows your terminal's foreground color. Decorative colors support true color and indexed color. The animation needs a font that renders Unicode braille characters correctly.

## Rendering measurements

Run `cargo run --release --locked -- demo --benchmark`. Add `--down` to measure the return timeline.

The benchmark measures geometry, braille encoding, and updates limited to changed cells. It excludes terminal drawing, input, labels, and synchronization sequences. Output volume depends on how much of the scene changes. These numbers describe this scene, not arbitrary terminal graphics.

On Linux x86_64 in this workspace, the release build produced these measurements:

| Terminal size | Median | 95th percentile | Average output | Estimated volume at 30 frames per second |
|---|---|---|---|---|
| 80x24 | 0.066 ms | 0.077 ms | 916 bytes/frame | 27 KiB/s |
| 120x40 | 0.093 ms | 0.103 ms | 1,681 bytes/frame | 49 KiB/s |
| 160x50 | 0.122 ms | 0.138 ms | 2,543 bytes/frame | 75 KiB/s |

The 160x50 case uses the capped 158x40 drawing region. Terminal rendering and remote connections can add latency. The renderer makes no graphics acceleration calls.

Pi's [terminal logo animation](https://github.com/earendil-works/pi/blob/v1.0.0/packages/coding-agent/src/modes/interactive/components/pi-logo-animation.ts) inspired the approach: colored braille dots, software lighting, reversible motion, and limited screen updates. Beam's implementation uses its own geometry and choreography. It remains internal until another effect establishes a useful shared interface.
