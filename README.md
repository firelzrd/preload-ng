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
- **Skip already-cached maps**: Running executables (score=0) were still included in the prefetch plan, wasting budget. Now breaks early when scores drop to zero.
- **Stable sorting with total_cmp**: Replaced `partial_cmp` with `total_cmp` for deterministic ordering regardless of NaN values.

### Broader monitoring coverage

- **Denylist instead of allowlist for prefix defaults**: The original only prefetched files under `/usr/`, `/lib/`, and `/var/cache/`. Switched to a denylist that excludes `/proc/`, `/sys/`, `/dev/`, `/tmp/`, and `/run/`, allowing applications in `/home/`, `/opt/`, and other paths to be prefetched.
- **fanotify file-open monitoring**: `/proc/[pid]/maps` only captures memory-mapped files. Added a `FAN_OPEN` watcher to discover files accessed via `read()` (icons, themes, configs, etc.). Falls back gracefully when `CAP_SYS_ADMIN` is unavailable.

### Reliable prefetching

- **Replace posix_fadvise(WILLNEED) with actual read()**: `WILLNEED` is an async hint the kernel may ignore under memory pressure. Switched to 128 KiB chunked `read()` calls to guarantee data lands in the page cache. `POSIX_FADV_SEQUENTIAL` is still used for readahead optimization.
- **Purge stale maps on prefetch failure**: When a prefetch fails, the file is checked for existence and removed from the store if missing. Prevents accumulation of obsolete entries after package updates.

### Better defaults

- **Default state_path**: Defaults to `$XDG_CACHE_HOME/preload-ng/state.db` (typically `~/.cache/preload-ng/state.db`). Learned data persists across restarts without any configuration.

## Usage

See [GUIDE.md](./GUIDE.md).

## Configuration

An example configuration is available at [docs/config.example.toml](./docs/config.example.toml).

## Development

See [CONTRIBUTING.md](./CONTRIBUTING.md) and [DEVELOPING.md](./DEVELOPING.md).

## License

Apache-2.0
