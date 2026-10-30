# Clean-room process

These rules are mandatory for all contributions, human or AI.

## Never
- Use disassembly or decompiled output of any Microsoft binary.
- Look at leaked Windows source code.
- Paste code from a source whose provenance is unknown.

## Allowed sources
- Microsoft's public documentation (learn.microsoft.com, SDK/WDK docs).
- The *Windows Internals* books.
- mingw-w64 headers (struct layouts, constants; used via `bindgen`).
- Wine and ReactOS source. These are LGPL/GPL: record anything borrowed or
  adapted in `THIRD_PARTY.md` in the same commit.
- Black-box behavior observed on real Windows (running test programs and
  recording their results, error codes and side effects).

## How work flows
1. **Spec**: describe the behavior (a doc, or better a test). Wine's
   conformance tests, which are validated on real Windows, are the primary
   spec.
2. **Test**: tests run on real Windows *and* on this kernel; results are
   compared (differential testing).
3. **Implementation**: written only from the spec and tests, never from
   Windows internals.

## ABI boundary only
Compatibility lives at the user-mode ABI: syscall numbers, struct layouts
(TEB, PEB, KUSER_SHARED_DATA, ...), handle and object semantics as
applications observe them. Internal kernel design is our own; do not mirror
NT's internal structures or algorithms.
