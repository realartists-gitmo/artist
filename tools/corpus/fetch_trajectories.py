#!/usr/bin/env python3
"""Pull a stratified sample of agent trajectories for the labelling corpus.

Streamed through HuggingFace's datasets-server rather than downloaded whole:
the dataset is 1.1 GB / 80k rows and we want a few hundred trajectories, on a
machine whose RAM is the scarce resource.

**Stratified on `target`, deliberately.** `target` records whether the agent
actually solved the task. Curated SFT sets keep only successes, and successes
are the *worst* source of durable facts — a run that worked reveals nothing
about what the code refuses to do. The interesting facts ("this vendors headers
that don't compile", "this leg is conjunctive", "this index is not
deterministic") come from things going wrong. So we take failures at parity
with successes, not in proportion to them.

What "real" means here, checked before choosing this dataset: the row carries a
`trajectory` list of ~90 messages with `role`/`text`, not a single request. Two
other candidates that looked right by name were not — Long-Horizon-Terminal-Bench
is 46 rows of Docker task definitions, and the merged Claude Code traces are
dominated by the harness's own sidecar calls (haiku generating conversation
titles), with an empty `claude_log`.
"""

from __future__ import annotations

import argparse
import json
import time
import urllib.parse
import urllib.request
from pathlib import Path

SERVER = "https://datasets-server.huggingface.co"
UA = "artist-corpus/0.1"


def get(url: str, tries: int = 4) -> dict:
    for attempt in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=90) as fh:
                return json.load(fh)
        except Exception as exc:  # noqa: BLE001 - retry anything transient
            if attempt == tries - 1:
                raise
            wait = 2 ** attempt
            print(f"  retry {attempt+1}/{tries} in {wait}s ({type(exc).__name__})", flush=True)
            time.sleep(wait)
    raise RuntimeError("unreachable")


def fetch_where(dataset: str, config: str, split: str, where: str, want: int, page: int):
    """Page through /filter, yielding rows until `want` distinct instances land.

    **Offsets are spread, and instances deduped.** Sequential paging returns
    rows grouped by `instance_id` — the dataset holds many models' attempts at
    the same task — so a contiguous read of 20 rows gave 20 attempts at
    *one* SWE-bench issue. A corpus like that would produce a vocabulary
    describing one Python file.

    Striding across the split and keeping the first row per instance costs
    nothing extra and gets breadth, which is the only reason to sample at all.
    """
    total = get(
        f"{SERVER}/filter?dataset={urllib.parse.quote(dataset)}"
        f"&config={config}&split={split}&where={urllib.parse.quote(where)}"
        f"&offset=0&length=1"
    ).get("num_rows_total", 0)
    if not total:
        return
    stride = max(1, total // max(want, 1))
    print(f"  {where}: {total} rows, striding by {stride}", flush=True)

    seen: set[str] = set()
    probe = 0
    while len(seen) < want and probe < want * 6:
        offset = (probe * stride) % max(total - page, 1)
        url = (
            f"{SERVER}/filter?dataset={urllib.parse.quote(dataset)}"
            f"&config={config}&split={split}"
            f"&where={urllib.parse.quote(where)}"
            f"&offset={offset}&length={page}"
        )
        for entry in get(url).get("rows", []):
            row = entry["row"]
            key = row.get("instance_id")
            if key in seen:
                continue
            seen.add(key)
            yield row
            if len(seen) >= want:
                break
        probe += 1
        print(f"  {where}: {len(seen)}/{want} distinct instances", flush=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dataset", default="nebius/SWE-agent-trajectories")
    ap.add_argument("--config", default="default")
    ap.add_argument("--split", default="train")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--per-class", type=int, default=100, help="rows per target value")
    ap.add_argument("--page", type=int, default=10, help="rows per request; keep small, they are big")
    args = ap.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    written = 0
    turns = 0
    with args.out.open("w") as fh:
        for where in ('"target"=true', '"target"=false'):
            for row in fetch_where(
                args.dataset, args.config, args.split, where, args.per_class, args.page
            ):
                traj = row.get("trajectory") or []
                turns += len(traj)
                fh.write(json.dumps(row) + "\n")
                written += 1

    size = args.out.stat().st_size / 1e6
    print(f"\nwrote {written} trajectories, {turns} messages, {size:.1f} MB -> {args.out}")
    print(f"mean {turns/max(written,1):.1f} messages per trajectory")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
