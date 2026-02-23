# preload-ng

The [preload-rs](https://github.com/arunanshub/preload-rs)-fork that I wanted.

preload-ng is a daemon that monitors running processes, predicts which applications will be launched next using a Markov chain, and prefetches their binaries and shared libraries into the page cache.  
This fork refines memory accounting, improves the prediction model, adds fanotify-based file monitoring, and makes prefetching reliable out of the box.

## Changes from upstream

### Refined memory budget calculation

- **Use MemAvailable instead of memfree/memcached**: Consolidated the separate `memfree` and `memcached` components into the kernel's single `MemAvailable` metric, avoiding potential double-counting of reclaimable memory.
- **Align units between procfs and planner**: The procfs crate returns bytes while the budget planner expects KB. Added the conversion so budget constraints behave as intended.

### Improved prediction model

- **Conservative fallback for insufficient correlation data**: The original returned `corr=0.0` when statistical data was sparse, which zeroed out prediction scores for newly observed executables. Now defaults to `f32::MIN_POSITIVE` so that Markov predictions are not over-weighted without evidence, while the base-probability floor (`1e-6`) and usage-frequency score still keep every observed executable as a prefetch candidate.
- **Score all observed exes by usage frequency**: Executables without Markov edges previously scored zero and were never prefetched. Added a base probability from `total_running_time / model_time` with a minimum floor, so every observed executable becomes a candidate.
- **Filter out zero-score maps**: Running executables receive score=0, which propagated to their maps. These zero-score maps were still included in the prefetch plan, wasting budget. Now filtered out before sorting so only positive-score candidates enter the plan.
- **Stable sorting with total_cmp**: Replaced `partial_cmp` with `total_cmp` for deterministic ordering regardless of NaN values.
- **Fix transition probability decay for unobserved states**: The branchless SIMD loop was applying `mix_tp` to all rows of the transition probability matrix on every state change, causing unobserved rows to lose probability mass over time. Now only the observed row uses `mix_tp`; other rows remain unchanged.

### Broader monitoring coverage

- **Denylist instead of allowlist for prefix defaults**: The original only prefetched files under `/usr/`, `/lib/`, and `/var/cache/`. Switched to a denylist that excludes `/proc/`, `/sys/`, `/dev/`, `/tmp/`, and `/run/`, allowing applications in `/home/`, `/opt/`, and other paths to be prefetched.
- **fanotify file-open monitoring**: `/proc/[pid]/maps` only captures memory-mapped files. Added a `FAN_OPEN` watcher to discover files accessed via `read()` (icons, themes, configs, etc.). Falls back gracefully when `CAP_SYS_ADMIN` is unavailable.

### Reliable prefetching

- **Switchable prefetch backends**: The original used `posix_fadvise(WILLNEED)`, an async hint the kernel may ignore under memory pressure. Now offers three backends—`readahead(2)` (async, no userspace buffer copy), `mmap+madvise(MADV_WILLNEED)`, and 128 KiB chunked `read()` (guaranteed page-cache fill)—with an `auto` mode (default) that probes and selects the fastest available. All backends apply `POSIX_FADV_SEQUENTIAL` for readahead optimization.
- **Skip cached pages via mincore(2)**: Before prefetching, pages already resident in memory are detected and skipped, eliminating redundant I/O.
- **Purge stale maps on prefetch failure**: When a prefetch fails, the file is checked for existence and removed from the store if missing. Prevents accumulation of obsolete entries after package updates.
- **Purge maps denied by mapprefix policy on startup**: Maps persisted in the state DB were loaded unconditionally, so changing the mapprefix exclusion list had no effect until prefetch failure. Now the admission policy is applied immediately after restoring the snapshot.

### Performance optimizations

- **Differential procfs scanning**: Only re-reads `/proc/[pid]/maps` when the process set changes, reducing read syscalls by ~80%.
- **FxHashMap/FxHashSet**: Replaced standard HashMap/HashSet with FxHash variants for 2–5x faster hashing on integer keys.
- **Arc\<Path\> interning**: Shared path references use `Arc<Path>` for O(1) clone instead of copying `PathBuf`.
- **SIMD-friendly Markov updates**: Branchless transition probability updates, 4-lane parallel accumulators for map score aggregation, and AoS-to-SoA layout conversion for cache efficiency.
- **fast_exp_neg()**: Range-reduction + 5th-order polynomial approximation (~20 ns → ~3–5 ns per call).
- **fp16 storage**: MarkovEdge fields (`time_to_leave`, `transition_prob`) and Prediction scores stored as f16, halving per-entry memory. F16C SIMD batch conversion via the `half` crate; f32 serialization in the persistence layer for backward compatibility.
- **SQLite persistence tuning**: Batch INSERT with prepared statement reuse, WAL mode, `synchronous=NORMAL`, `mmap_size=256 MB`, `cache_size=8 MB`.
- **target-cpu=native build**: Enables AVX2/FMA/BMI2 instruction sets via `.cargo/config.toml`.
- **Poll-based fanotify**: Replaced fixed-sleep polling with `poll(2)` for lower latency event delivery.

### Better defaults

- **Default state_path**: Defaults to `$XDG_CACHE_HOME/preload-ng/state.db` (typically `~/.cache/preload-ng/state.db`). Learned data persists across restarts without any configuration.
- **Lower default minsize from 2 MB to 100 KB**: The original 2 MB threshold excluded statically-linked Go/Rust binaries and smaller GUI apps. 100 KB filters out trivial utilities while covering more real applications.
- **nice(19) & ionice IDLE class**: The daemon automatically sets CPU priority to the lowest (`nice 19`) and I/O scheduling to `IOPRIO_CLASS_IDLE` at startup, ensuring prefetch I/O only runs when no other process needs disk.
- **prefetch_concurrency = 1**: Default to single-worker prefetch to minimize I/O contention. Comment out to use all CPU cores.

### Operational improvements

- **Graceful shutdown on Ctrl+C during sleep**: The inter-tick sleep is now wrapped in `tokio::select!` with a cancellation token, so SIGINT is handled immediately instead of waiting for the cycle to complete.

## Usage

See [GUIDE.md](./GUIDE.md).

## Configuration

An example configuration is available at [docs/config.example.toml](./docs/config.example.toml).

## Development

See [CONTRIBUTING.md](./CONTRIBUTING.md) and [DEVELOPING.md](./DEVELOPING.md).

## License

Apache-2.0
