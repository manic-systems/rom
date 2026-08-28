# `nixpkgs#hello` fixture

This is a compact, deterministic replay sampled from a real Lix build of
`nixpkgs#hello` (`hello-2.12.3`). It retains the real derivation lifecycle,
representative build lines, and Lix's colored `PASS` output. Repetitive lines
from the 1,936-line daemon log are intentionally omitted so visual regressions
remain reviewable.

The timestamps preserve a plausible ordering for interactive playback; they
are not measurements of the original build.
