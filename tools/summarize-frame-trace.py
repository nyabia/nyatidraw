"""Bounded frame-trace report: python tools/summarize-frame-trace.py CAMPAIGN_DIR [OUTPUT.json].

JSON goes to stdout or OUTPUT.json; exit 0 means validated accounting/verification, 2 means invalid.
No percentile subtraction, cross-run averaging, or artwork/visible-pixel inference.
"""
import argparse
import json
import re
import sys
from pathlib import Path


MARKS = "render scene_begin scene_end acquire_begin acquire_end present_begin present_end".split()
OFFSET_KEY = "offsets_us_render_scene_begin_scene_end_acquire_begin_acquire_end_present_begin_present_end"
FIELDS = re.compile(r"(\w+)=((?:\[[^\]]*\])|(?:Some\([^)]*\))|(?:\S+))")
MATERIALIZED = ["materialized_" + name for name in (
    "us calls completed cpu_clones cpu_bytes upload_attempts upload_successes deferred outside_scene_calls".split())]


def materialized_fields(row, errors):
    for key in MATERIALIZED:
        row.setdefault(key, None)  # Old trace version did not observe these values.
    values = [row[key] for key in MATERIALIZED]
    if all(value is None for value in values):
        return
    if any(type(value) is not int or value < 0 for value in values):
        errors.append("incomplete/invalid materialization counters")
        return
    if row["materialized_upload_successes"] > row["materialized_upload_attempts"] or row["materialized_outside_scene_calls"] > row["materialized_calls"]:
        errors.append("inconsistent materialization counters")


def typed(value):
    if value == "None":
        return None
    if value in ("true", "false"):
        return value == "true"
    if value.startswith("Some(") and value.endswith(")"):
        return typed(value[5:-1])
    if value.startswith("[") and value.endswith("]"):
        return [typed(part.strip()) for part in value[1:-1].split(",")]
    return int(value) if re.fullmatch(r"-?\d+", value) else value


def fields(line):
    pairs = FIELDS.findall(line.partition(" ")[2])
    if len({key for key, _ in pairs}) != len(pairs):
        raise ValueError("duplicate trace fields")
    return {key: typed(value) for key, value in pairs}


def frame(line):
    row = fields(line)
    offsets = row.pop(OFFSET_KEY)
    if len(offsets) != len(MARKS) or any(type(v) is not int for v in offsets if v is not None):
        raise ValueError("invalid boundary offsets")
    row["offsets_us"] = dict(zip(MARKS, offsets))
    errors = []
    ordered = [0] + [v for v in offsets if v is not None] + [row["end_us"]]
    if any(end < begin for begin, end in zip(ordered, ordered[1:])):
        errors.append("nonmonotonic actor boundaries")
    row["durations_us"] = {}
    for name in ("scene", "acquire", "present"):
        begin, end = (row["offsets_us"][f"{name}_{edge}"] for edge in ("begin", "end"))
        row["durations_us"][name] = end - begin if begin is not None and end is not None and end >= begin else None
    present = row["offsets_us"]["present_end"]
    expected = "present-request-returned" if present is not None else (
        "render-without-present" if offsets[0] is not None else "control-only")
    if row["status"] != expected:
        errors.append("status disagrees with boundaries")
    admission = row["admission_offset_us"]
    first, last = row["first_dequeue_offset_us"], row["last_dequeue_offset_us"]
    if admission is not None and (first is None or last is None or not admission <= first <= last <= row["end_us"]):
        errors.append("invalid admission/dequeue ordering")
    row["carried_input"] = row["input_first_batch"] is not None and row["input_first_batch"] < row["batch"]
    materialized_fields(row, errors)
    for key in ("viewport_calls", "viewport_last_begin_us", "viewport_last_end_us"):
        row.setdefault(key, None)
    begin, end = row["viewport_last_begin_us"], row["viewport_last_end_us"]
    row["durations_us"]["viewport_last_call"] = end - begin if type(begin) is int and type(end) is int and end >= begin else None
    if begin is not None and (type(begin) is not int or (end is not None and (type(end) is not int or end < begin))):
        errors.append("invalid viewport boundary ordering")
    if end is not None and begin is None:
        errors.append("viewport end without begin")
    scene_begin, scene_end = row["offsets_us"]["scene_begin"], row["offsets_us"]["scene_end"]
    row["viewport_last_inside_scene"] = (scene_begin <= begin <= end <= scene_end
        if all(type(v) is int for v in (scene_begin, begin, end, scene_end)) else None)
    if row["materialized_outside_scene_calls"] == 0 and row["materialized_calls"]:
        scene_us = row["durations_us"]["scene"]
        if scene_us is not None and row["materialized_us"] > scene_us + 1:
            errors.append("materialization accumulated time exceeds containing scene")
    if row["input_first_batch"] is not None and row["input_first_batch"] > row["batch"]:
        errors.append("input comes from a future batch")
    if (present is None or admission is None) and row["admission_to_present_us"] is not None:
        errors.append("latency without admission/present")
    latency = row["admission_to_present_us"]
    if present is not None and admission is not None and (
            type(latency) is not int or abs(latency - (present - admission)) > 1):
        errors.append("admission-to-present disagrees with offsets beyond 1us rounding")
    row["instrumentation_errors"] = errors
    return row


def summarize(root):
    metadata_path = root / "metadata.json"
    report = {"metadata_source": metadata_path.relative_to(root).as_posix(),
              "metadata": json.loads(metadata_path.read_text(encoding="utf-8-sig")),
              "limits": "First qualifying retained rows, not unbiased samples or full distributions. Wake is mailbox observation, not producer wake. Signed offsets may carry older input across skipped/control batches. Helper threads are unobserved. API correlation is not artwork or visible-pixel proof.",
              "runs": [], "excluded_runs": [], "errors": []}
    excluded = report["metadata"].get("excluded_runs", {})
    if not isinstance(excluded, dict):
        raise ValueError("excluded_runs must map run names to explicit reasons")
    for name, reason in excluded.items():
        if not re.fullmatch(r"[A-Za-z][A-Za-z_-]*-\d+", name) or not isinstance(reason, str) or not reason.strip():
            raise ValueError("excluded_runs requires MODE-N names and nonempty reasons")
        log = root / name / "out.log"
        if not log.is_file():
            raise ValueError(f"excluded run source missing: {name}/out.log")
        report["excluded_runs"].append({"run": name, "reason": reason,
                                        "source": log.relative_to(root).as_posix()})
    for log in sorted(root.glob("*/out.log")):
        if not re.fullmatch(r"[A-Za-z][A-Za-z_-]*-\d+", log.parent.name):
            continue
        if log.parent.name in excluded:
            continue
        text = log.read_text(encoding="utf-8-sig")
        rows = [frame(line) for line in text.splitlines() if line.startswith("performance-frame ")]
        summaries = [fields(line) for line in text.splitlines() if line.startswith("performance-frames ")]
        errors = []
        if len(summaries) != 1:
            errors.append("expected exactly one actor lifetime summary")
        summary = summaries[0] if len(summaries) == 1 else {}
        if summary:
            materialized_fields(summary, errors)
            for key in MATERIALIZED:
                if type(summary[key]) is int and summary[key] < sum(row[key] or 0 for row in rows):
                    errors.append(f"retained {key} exceeds lifetime total")
            counts = "batches recorded omitted below_threshold presented skipped control_only capacity pending_unpresented_batches unframed_dequeues_in_recorded_thread coalesced_presentations".split()
            if any(type(summary[key]) is not int or summary[key] < 0 for key in counts):
                errors.append("invalid nonnegative summary counts")
            if summary["recorded"] + summary["omitted"] + summary["below_threshold"] != summary["batches"]:
                errors.append("retention accounting mismatch")
            if summary["presented"] + summary["skipped"] + summary["control_only"] != summary["batches"]:
                errors.append("outcome accounting mismatch")
            if summary["recorded"] != len(rows) or not len(rows) <= summary["capacity"] <= 128:
                errors.append("retained row count/capacity mismatch")
            if summary["selection"] != "first-qualifying" or summary["wake"] != "mailbox-observed":
                errors.append("unknown selection/wake semantics")
            if any(row["owner"] != summary["owner"] or not 1 <= row["batch"] <= summary["batches"] for row in rows):
                errors.append("row owner/batch outside summary")
        if len({row["batch"] for row in rows}) != len(rows):
            errors.append("duplicate retained batch IDs")
        if [row["batch"] for row in rows] != sorted(row["batch"] for row in rows):
            errors.append("retained batch IDs out of order")
        if any(row["instrumentation_errors"] for row in rows):
            errors.append("invalid row timing; inspect instrumentation_errors")
        verifier = next((log.with_name(name) for name in ("verifier.log", "verify.log") if log.with_name(name).exists()), None)
        verified = verifier is not None and "tiles=exact png=exact" in verifier.read_text(encoding="utf-8-sig")
        if not verified or "event=close-ready writer=joined" not in text:
            errors.append("missing reopen/export verification or normal close")
        if not re.search(r"performance-probe event=complete mode=\w+ strokes=32 samples_per_stroke=121", text):
            errors.append("missing complete workload marker")
        tails = sorted((row for row in rows if row["admission_to_present_us"] is not None),
                       key=lambda row: row["admission_to_present_us"], reverse=True)[:10]
        report["runs"].append({"run": log.parent.name, "source": log.relative_to(root).as_posix(),
                               "verifier_source": verifier.relative_to(root).as_posix() if verifier else None,
                               "summary": summary, "rows": rows, "top_retained_input_tails": tails,
                               "valid": not errors, "errors": errors})
    if not report["runs"]:
        report["errors"].append("no MODE-N/out.log runs")
    report["valid"] = not report["errors"] and all(run["valid"] for run in report["runs"])
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("campaign_dir", type=Path)
    parser.add_argument("output", nargs="?", type=Path)
    args = parser.parse_args()
    try:
        result = summarize(args.campaign_dir)
    except (OSError, ValueError, KeyError, TypeError) as error:
        result = {"valid": False, "errors": [str(error)]}
    encoded = json.dumps(result, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        try:
            args.output.write_text(encoded, encoding="utf-8")
        except OSError as error:
            print(f"frame-report valid=false output-write-error={error}", file=sys.stderr)
            sys.exit(2)
        print(f"frame-report valid={str(result['valid']).lower()} runs={len(result.get('runs', []))} output={args.output}")
    else:
        print(encoded, end="")
    sys.exit(0 if result["valid"] else 2)
