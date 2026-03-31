watch-build:
    #!/usr/bin/env bash
    set -euo pipefail

    python3 <<'PY'
    import subprocess
    import time
    from pathlib import Path

    WATCH_DIRS = [Path("rust"), Path("tests"), Path("proto")]
    WATCH_FILES = [Path("Cargo.toml"), Path("Cargo.lock"), Path("build.rs")]
    WATCH_SUFFIXES = {".rs", ".proto"}

    def snapshot() -> tuple[tuple[str, int, int], ...]:
        entries = []
        for directory in WATCH_DIRS:
            if not directory.exists():
                continue
            for path in sorted(
                candidate
                for candidate in directory.rglob("*")
                if candidate.is_file() and candidate.suffix in WATCH_SUFFIXES
            ):
                stat = path.stat()
                entries.append((str(path), stat.st_mtime_ns, stat.st_size))
        for path in WATCH_FILES:
            if path.exists():
                stat = path.stat()
                entries.append((str(path), stat.st_mtime_ns, stat.st_size))
        return tuple(entries)

    last = None
    print("Watching himitsu sources; press Ctrl-C to stop.")
    while True:
        current = snapshot()
        if current != last:
            last = current
            print("\n==> cargo build --bin himitsu")
            result = subprocess.run(["cargo", "build", "--bin", "himitsu"], check=False)
            if result.returncode != 0:
                print(f"cargo build failed with exit code {result.returncode}")
        time.sleep(0.5)
    PY
