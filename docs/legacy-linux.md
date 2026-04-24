# Legacy Linux Workflow

This project is aimed at workloads where page locality and decode latency
matter more than peak whole-buffer throughput, especially on older laptops.

## Recommended Capture Flow

1. Collect page samples outside the repository.
2. Split them into one `*.bin` file per page.
3. Train a profile with `sxrc-train`.
4. Benchmark both the default and trained manifests with `sxrc-bench`.
5. Export JSON so the results can be diffed or summarized later.

Example:

```bash
cargo run --release --bin sxrc-train -- \
  --input-dir ./corpus/pages \
  --output ./trained.sxrc.yaml

cargo run --release --bin sxrc-bench -- \
  --manifest ./trained.sxrc.yaml \
  --input-dir ./corpus/pages \
  --mode zram \
  --iterations 8 \
  --warmup 2 \
  --page-size 4096 \
  --baseline both \
  --latency-stats \
  --export-json ./reports/legacy-linux.json
```

## What To Compare

- `ratio`
- `encode_mib_s`
- `decode_mib_s`
- `decode_p95_us`
- `decode_p99_us`
- `zram_zero_pages`
- `zram_same_pages`
- `zram_sxrc_pages`
- `zram_raw_pages`

## Suggested Corpus Classes

- Idle desktop / session baseline
- Browser-heavy interactive use
- Emulator, code, or AOT-heavy repeated-page workloads

## Reporting Template

For each corpus, record:

- machine and kernel version
- manifest used (`default` or trained path)
- page size
- iterations and warmup
- whether SXRC beat raw fallback on enough pages
- whether decode `p95`/`p99` improved or regressed versus `zstd` and `lz4`

This repository does not ship real machine dumps or a canned E725 report.
Capture those locally and keep only sanitized JSON summaries or written
conclusions in version control.
