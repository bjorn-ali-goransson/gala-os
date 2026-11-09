# web-platform-tests on gala

The `chromium_wpt` run's results, recorded for comparison round to round.

- `dom-nodes.txt`: one line per test (`<path>: OK 9/9`), the summary last, as
  the run's serve log has them.
- `dom-nodes-compare.txt`: `cargo xtask wpt-compare`'s output -- gala against
  wpt.fyi's pinned runs, and every test that passes fewer subtests here than on
  `edge-159-win`, with its first failing subtest.

The wptrunner-format report itself (`target/wpt/wptreport.json`, 1.6 MB that
changes wholly each run) stays a build artifact: WPT's commit, Chromium's
version and the session's flags are recorded here and in `xtask/src/wpt.rs`, so
a run reproduces it.

## The recorded run

`cargo xtask boot-test chromium_wpt --release` under KVM, 263 s of kernel time:
WPT `dd216d1d4f61489b09f566bec2c52d229baa7f49`, Chromium 159.0.8140.0, headless,
one ChromeDriver and one browser session for the whole directory.

**406 tests, 404 harness OK, 13326 of 13397 subtests pass.** wpt.fyi's runs of
the same Chromium version: `edge-159-win` 13343, `chrome-159-linux` 13349.

Seven tests pass fewer subtests here than on `edge-159-win`:

- Five are this harness, not gala: `moveBefore/hover-style-update`,
  `moveBefore/pointer-events`, `moveBefore/selection-preserve` and
  `moveBefore/role-updates-after-move` need testdriver's `action_sequence` and
  `get_computed_role`, which our testdriver-vendor.js does not implement, and
  `insertion-removing-steps/Node-append-meta-referrer-and-script-from-fragment`
  asserts the referrer a script sees in its post-insertion steps.
- `moveBefore/relevant-mutations` is a TIMEOUT (2 subtests).
- `Document-createEvent.https` misses 6 subtests `edge-159-win` passes, all
  constructors of sensor events (`DeviceMotionEvent` and the like); wptrunner
  passes `--enable-features=GenericSensorExtraClasses`, which this session does
  not, so the flags are the first thing to rule out.
