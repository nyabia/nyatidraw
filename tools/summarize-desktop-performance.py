"""Collect installed desktop timing rows without averaging percentiles across runs.

Usage: python tools/summarize-desktop-performance.py CAMPAIGN_DIR OUTPUT.json
Supply hardware/build/workload context in CAMPAIGN_DIR/metadata.json.
"""
import json
import re
import sys
from pathlib import Path


def summarize(root: Path) -> dict:
    runs = []
    for log in sorted(root.glob("*/out.log")):
        text = log.read_text(encoding="utf-8-sig")
        errors = log.with_name("err.log").read_text(encoding="utf-8-sig")
        verified = log.with_name("verify.log").read_text(encoding="utf-8-sig")
        completion = re.findall(r"performance-probe event=complete mode=(\w+) strokes=(\d+) samples_per_stroke=(\d+)", text)
        if len(completion) != 1 or completion[0][1:] != ("32", "121"):
            raise ValueError(f"{log}: incomplete workload")
        if "event=stopped" in errors or "event=start-failed" in errors:
            raise ValueError(f"{log}: driver failed")
        if "event=close-ready writer=joined" not in text or "tiles=exact png=exact" not in verified:
            raise ValueError(f"{log}: missing normal close or reopen/export verification")
        strokes = re.findall(r"event=stroke-closed count=\d+ samples=(\d+) first_sequence=(\d+) last_sequence=(\d+)", text)
        if strokes != [("121", str(i * 121 + 1), str((i + 1) * 121)) for i in range(32)]:
            raise ValueError(f"{log}: input sample sequence differs")
        rows = []
        for line in text.splitlines():
            if line.startswith("performance owner="):
                row = dict(part.split("=", 1) for part in line.split()[1:])
                rows.append({k: int(v) if v.isdigit() else v for k, v in row.items()})
        lateness = [int(v) for v in re.findall(r"max_schedule_lateness_us=(\d+)", text)]
        accounting = []
        for phase in ("inactive", "active"):
            counts = {
                stage: sum(row.get("count", 0) for row in rows
                           if row.get("stage") == stage and row.get("export") == phase)
                for stage in ("input_batch_to_dequeue", "input_batch_to_present_request")
            }
            dequeued = counts["input_batch_to_dequeue"]
            presented = counts["input_batch_to_present_request"]
            accounting.append({"export": phase, "dequeued_batches": dequeued,
                               "present_request_samples": presented,
                               "difference": dequeued - presented})
        runs.append({
            "run": log.parent.name, "mode": completion[0][0],
            "input": "synthetic-paced-direct-admission", "strokes": 32, "samples": 3872,
            "max_schedule_lateness_us": max(lateness),
            "explicit_start_marker": "event=awaiting-start" in text,
            "reopen_export_verified": True, "stderr": errors.strip(), "rows": rows,
            "input_accounting": accounting,
            "pending_batch_at_flush": sum(row.get("input_batch_without_present", 0) for row in rows),
        })
    if not runs:
        raise ValueError("no run logs")
    return {"metadata": json.loads((root / "metadata.json").read_text(encoding="utf-8-sig")),
            "percentiles": "per-run nearest-rank histogram upper bounds; no cross-run averaging",
            "input_classification": "Export phase is latched at dequeue, not admission. InputPresent retains the oldest pending batch until a successful present API return; skipped frames may coalesce multiple dequeues. Counts are reported, not assumed equal. Close/SaveAs tail work on unflushed helper threads is outside these steady-state distributions.",
            "runs": runs}


if __name__ == "__main__":
    campaign, output = map(Path, sys.argv[1:])
    report = summarize(campaign)
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"performance-report runs={len(report['runs'])} output={output}")
