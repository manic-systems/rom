# ROM Display Formats

Unlike prior art, ROM features several different display and legend formats as
opposed to NOM's immutable design. This allows for the freedom to mix and match
different component styles in the build graph.

## Display Formats

ROM supports three display formats controlled by the `--format` flag:

1. Tree Format (Default)
2. Plain Format
3. Dashboard Format

### 1. Tree Format (Default)

The tree format shows a hierarchical dependency graph with build progress.

**Usage:**

```bash
rom --format tree build nixpkgs#hello
# or simply (tree is default)
rom build nixpkgs#hello
```

### Examples

**Tree Format**:

```plaintext
╭─ Builds
├─ ⏸ hello-2.12.2
│  ╰─ ⏵ dependency-1.0  ⏱ 5s
┢━ Status      Running   Completed   Waiting   Failed   Total
┃  Builds      ⏵ 1       ✔ 0         ⏸ 4       ✗ 0      ∑ 5
┃  Downloads   ↓ 2       ↓ 3         ⏸ 1                ∑ 6
┗━ Elapsed ⏱ 5s
```

**Plain Format**:

```plaintext
─ Builds  ⏱ 5s  ⏵ 1 building  ⏸ 4 planned
  ⏵ hello-2.12.2  5s
  breakpad-2024.02.16  ↓ ━━━━╸───────────────  24%  1.2 MiB/5.0 MiB
```

**Dashboard Format**:

```plaintext
╭─ Build Dashboard: hello-2.12.2
│  Host      │ localhost
│  Status    │ ⏵ building
│  Duration  │ 8s
╰─ Summary   │ jobs=5  ok=0  failed=0  waiting=4
```

## Legend Styles

Legend styles control how the build statistics are displayed at the bottom of
the screen. At this moment they only affect the **tree format**.

1. Table Style
2. Compact Style
3. Verbose Style
4. Nom Style

### Examples

**Table**:

```plaintext
╭─ Builds
├─ ⏵ hello-2.12.2  ⏱ 5s
┢━ Status      Running   Completed   Waiting   Failed   Total
┃  Builds      ⏵ 1       ✔ 0         ⏸ 4       ✗ 0      ∑ 5
┃  Downloads   ↓ 2       ↓ 3         ⏸ 1                ∑ 6
┗━ Elapsed ⏱ 5s
```

**Compact**:

```plaintext
╭─ Builds
├─ ⏵ hello-2.12.2  ⏱ 5s
┕━  ⏵ 1 ✔ 0 ✗ 0 ⏸ 4
```

**Verbose**:

```plaintext
╭─ Builds
├─ ⏵ hello-2.12.2  ⏱ 5s
┢━ Build Summary
┃  ⏵ hello-2.12.2  5s
┗━  ⏵ 1 building ✔ 0 completed ✗ 0 failed ⏸ 4 waiting
```

**Nom**:

```plaintext
╭─ ⏸ nixos-system-laptop-26.11.20261113.d5cc683
┢━━━ Builds           │ Downloads                              │ Host
┃    ⏵ 1 │     │ ⏸ 21 │     │     │     │                      │ localhost
┃        │     │      │ ↓ 1 │ ↓ 3 │     │ ↓ 88.9 MiB/334.9 MiB │ cache.nixos.org (https)
┃        │     │      │ ↓ 1 │     │     │   ↓ 70.6 MiB/1.3 GiB │ download.jetbrains.com (https)
┗━ ∑ ⏵ 1 │ ✔ 0 │ ⏸ 21 │ ↓ 2 │ ↓ 3 │ ⏸ 0 │  ↓ 159.5 MiB/1.6 GiB │ ⏱ 19s
```

Host rows in the `nom` legend (and matching `--summary nom`) can be sorted with
`--host-sort` (`none`, `download-size`, `download-count`, `builds`) and capped
with `--max-hosts <N>`, which combines overflow hosts into an `(other)` row.

## Icon Legend

All formats use consistent icons (default colors shown; any color can be
customized via `[theme]` in `config.toml`):

| Icon | Meaning           | Color   |
| ---- | ----------------- | ------- |
| ⏵    | Building/Running  | Yellow  |
| ✔    | Completed/Success | Green   |
| ✗    | Failed/Error      | Red     |
| ⏸    | Planned/Waiting   | Blue    |
| ⏱    | Time/Duration     | Grey    |
| ↓    | Downloading       | Cyan    |
| ↑    | Uploading         | Magenta |
