#!/usr/bin/env python3
"""Observe fresh crates.io yank status for a frozen audit's shipping graphs.

Python 3.11+; no third-party packages. One unconditional HTTPS sparse-index GET
per unique crate, plus config.json. Never reads Cargo's registry cache. The
combined gate preserves the input advisory gate: 0=both clean, 1=findings,
2=unavailable/incomplete. Outputs refuse overwrite. See --help and audit README.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import datetime
import gzip
import hashlib
import http.client
import json
from pathlib import Path
import re
import sys
import threading
import time
import tomllib
import urllib.error
import urllib.request


TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc")
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
INDEX = "https://index.crates.io/"
MAX_RESPONSE = 8 * 1024 * 1024
MAX_TOTAL = 64 * 1024 * 1024
MAX_SECONDS = 600
REQUEST_INTERVAL = 1.05
SOCKET_TIMEOUT = 20
HEADERS = {
    "User-Agent": "ReLinkDependencyAudit/1.0",
    "Accept": "text/plain, application/json",
    "Accept-Encoding": "identity",
    "Cache-Control": "no-cache, max-age=0",
    "Pragma": "no-cache",
}
RETAIN_HEADERS = (
    "date", "etag", "last-modified", "cache-control", "age", "content-type",
    "content-length", "content-encoding", "retry-after", "via", "x-cache", "x-cache-hits",
)


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def index_path(name):
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", name):
        raise ValueError("invalid crates.io package name")
    name = name.lower()
    if len(name) < 3:
        return f"{len(name)}/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def load_scope(audit):
    """Validate captured bytes before deriving an exact name/version scope."""
    hashes = json.loads((audit / "SHA256SUMS.json").read_text(encoding="utf-8"))
    required = ["Cargo.lock.snapshot", "input.json", "summary.json"]
    required += [f"{target}-{label}.txt" for target in TARGETS for label in ("app-all", "app-runtime", "hal-all")]
    contents = {}
    for name in required:
        data = (audit / name).read_bytes()
        if hashes.get(name) != sha(data):
            raise ValueError(f"audit evidence checksum mismatch: {name}")
        contents[name] = data
    source_input = json.loads(contents["input.json"])
    if source_input["lockfile_sha256"] != sha(contents["Cargo.lock.snapshot"]):
        raise ValueError("input metadata and lockfile hashes differ")
    summary = json.loads(contents["summary.json"])
    if type(summary.get("audit_exit_code")) is not int or summary["audit_exit_code"] not in (0, 1):
        raise ValueError("input advisory audit was unavailable")
    packages = tomllib.loads(contents["Cargo.lock.snapshot"].decode("utf-8"))["package"]
    by_key = {}
    for package in packages:
        by_key.setdefault((package["name"], package["version"]), []).append(package)
    membership = {}
    for target in TARGETS:
        for label in ("app-all", "app-runtime", "hal-all"):
            lines = contents[f"{target}-{label}.txt"].decode("utf-8").splitlines()
            keys = set()
            for line in lines:
                match = re.fullmatch(r"([\w-]+) v([^\s]+)(?: .*)?", line)
                if not match:
                    raise ValueError("unexpected Cargo graph line")
                keys.add(match.groups())
            if not keys:
                raise ValueError("empty shipping dependency graph")
            membership[(target, label)] = keys
    union = set().union(*(membership[(target, label)] for target in TARGETS for label in ("app-all", "hal-all")))
    selected, excluded = [], []
    for key in sorted(union):
        matches = by_key.get(key, [])
        # A text graph cannot distinguish equal name/version from two sources.
        # Refuse ambiguity rather than attributing a private crate to crates.io.
        if len(matches) != 1:
            raise ValueError(f"ambiguous or missing lockfile identity: {key}")
        package = matches[0]
        if package.get("source") != REGISTRY:
            excluded.append({"name": key[0], "version": key[1], "reason": "not a crates.io registry package"})
            continue
        index_path(package["name"])
        if not re.fullmatch(r"[0-9a-f]{64}", package.get("checksum", "")):
            raise ValueError("missing or invalid locked registry checksum")
        selected.append({"name": key[0], "version": key[1], "checksum": package["checksum"],
                         "targets": {target: {label: key in membership[(target, label)] for label in ("app-all", "app-runtime", "hal-all")} for target in TARGETS}})
    if not selected or len(selected) > 2000:
        raise ValueError("shipping registry package count outside safety bounds")
    provenance = {"source_audit_directory": str(audit), "source_files_sha256": {name: sha(data) for name, data in contents.items()},
                  "lockfile_sha256": source_input["lockfile_sha256"], "advisory_summary": summary,
                  "selected_packages": selected, "excluded_packages": excluded}
    return provenance, contents["Cargo.lock.snapshot"]


class Limits:
    """Shared pacing, stop signal and byte reservations; no unbounded retries."""
    def __init__(self, interval=REQUEST_INTERVAL, seconds=MAX_SECONDS, total=MAX_TOTAL):
        self.lock = threading.Lock()
        self.interval = interval
        self.deadline = time.monotonic() + seconds
        self.next_request = 0.0
        self.remaining = total
        self.received = 0
        self.stopped = None

    def stop(self, reason):
        with self.lock:
            self.stopped = self.stopped or reason

    def acquire(self):
        while True:
            with self.lock:
                current = time.monotonic()
                if current >= self.deadline:
                    self.stopped = self.stopped or "run deadline exceeded"
                if self.stopped:
                    return self.stopped
                delay = self.next_request - current
                if delay <= 0:
                    self.next_request = current + self.interval
                    return None
            time.sleep(min(delay, 0.25))

    def reserve(self, wanted):
        with self.lock:
            if time.monotonic() >= self.deadline:
                self.stopped = self.stopped or "run deadline exceeded"
            if self.stopped:
                raise ValueError(self.stopped)
            amount = min(wanted, self.remaining)
            if amount == 0:
                self.stopped = "total response byte budget exhausted"
                raise ValueError(self.stopped)
            self.remaining -= amount
            return amount

    def received_bytes(self, reserved, actual):
        with self.lock:
            self.remaining += reserved - actual
            self.received += actual


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch(name, path, output, limits, opener=None):
    """Keep each complete/partial response; only a complete HTTP 200 is usable."""
    result = {"name": name, "url": INDEX + path, "started_at_utc": now(),
              "status": "unavailable", "http_status": None, "headers": {}, "body_complete": False}
    stopped = limits.acquire()
    if stopped:
        result.update(reason=stopped, ended_at_utc=now())
        return result, None
    body = bytearray()
    try:
        opener = opener or urllib.request.build_opener(NoRedirect())
        request = urllib.request.Request(result["url"], headers=HEADERS)
        try:
            response = opener.open(request, timeout=SOCKET_TIMEOUT)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            result["http_status"] = response.status
            result["headers"] = {key: response.headers[key] for key in RETAIN_HEADERS if key in response.headers}
            if response.status == 429 or (response.status == 503 and response.headers.get("Retry-After")):
                # Do not retry or launch new requests during a server backoff.
                # Already in-flight requests may finish; all other items become unavailable.
                limits.stop(f"registry requested backoff (HTTP {response.status})")
            if response.headers.get("Content-Encoding", "identity").lower() != "identity":
                raise ValueError("unexpected response content encoding")
            while True:
                reserved = limits.reserve(min(65536, MAX_RESPONSE + 1 - len(body)))
                try:
                    chunk = response.read1(reserved)
                except Exception:
                    limits.received_bytes(reserved, 0)
                    raise
                limits.received_bytes(reserved, len(chunk))
                body.extend(chunk)
                if len(body) > MAX_RESPONSE:
                    raise ValueError("per-response byte budget exceeded")
                if not chunk:
                    break
            length = response.headers.get("Content-Length")
            if length is not None and int(length) != len(body):
                raise ValueError("truncated or inconsistent response length")
            result["body_complete"] = True
            if response.status != 200:
                raise ValueError(f"HTTP {response.status}")
            result["status"] = "observed"
    except (OSError, ValueError, urllib.error.URLError, http.client.HTTPException) as error:
        # No credentials or proxy URLs are copied from exception messages.
        result["reason"] = str(error) if isinstance(error, ValueError) else type(error).__name__
    result["ended_at_utc"] = now()
    result["body_bytes"] = len(body)
    result["body_sha256"] = sha(body)
    filename = f"responses/{name}.jsonl.gz"
    (output / filename).write_bytes(gzip.compress(bytes(body), mtime=0))
    result["body_file"] = filename
    write_json(output / f"responses/{name}.metadata.json", result)
    return result, bytes(body) if result["status"] == "observed" else None


def evaluate(name, packages, body):
    rows = [json.loads(line) for line in body.decode("utf-8", errors="strict").splitlines()]
    if not rows or any(not isinstance(row, dict) or row.get("name") != name for row in rows):
        raise ValueError("index contains an invalid or mismatched package record")
    results = []
    for package in packages:
        base = {"name": name, "version": package["version"], "locked_checksum": package["checksum"], "targets": package["targets"]}
        matches = [row for row in rows if row.get("vers") == package["version"]]
        equivalent = [row for row in rows if isinstance(row.get("vers"), str) and row["vers"].split("+", 1)[0] == package["version"].split("+", 1)[0]]
        if len(matches) != 1 or len(equivalent) != 1:
            results.append({**base, "status": "unavailable", "reason": "locked version missing or duplicated in index"})
            continue
        row = matches[0]
        if type(row.get("v", 1)) is not int or row.get("v", 1) not in (1, 2) or type(row.get("yanked")) is not bool:
            results.append({**base, "status": "unavailable", "reason": "unknown schema or invalid yanked field"})
            continue
        checksum = row.get("cksum")
        if not isinstance(checksum, str) or not re.fullmatch(r"[0-9a-f]{64}", checksum):
            results.append({**base, "status": "unavailable", "reason": "index checksum malformed"})
            continue
        status = "checksum_mismatch" if checksum != package["checksum"] else ("yanked" if row["yanked"] else "not_yanked")
        results.append({**base, "status": status, "yanked": row["yanked"], "index_checksum": checksum})
    return results


def summarize(results, advisory_exit):
    counts = {status: sum(item["status"] == status for item in results) for status in ("not_yanked", "yanked", "checksum_mismatch", "unavailable")}
    yank_exit = 2 if counts["unavailable"] else (1 if counts["yanked"] or counts["checksum_mismatch"] else 0)
    return {"counts": counts, "yank_exit_code": yank_exit, "advisory_exit_code": advisory_exit,
            "combined_exit_code": max(yank_exit, advisory_exit),
            "status": "unavailable" if yank_exit == 2 else ("findings" if yank_exit == 1 else "verified_not_yanked")}


def follow_up_scope(directory, scope):
    """Retain successful prior observations; never rewrite the failed run.

    A follow-up is explicitly composite and must begin within 15 minutes of the
    earliest retained observation. Its requests have the normal finite budget.
    Existing yank/checksum findings remain in the gate and are not rechecked away.
    """
    manifest = json.loads((directory / "SHA256SUMS.json").read_text(encoding="utf-8"))
    records = {}
    hashes = {}
    for name in ("input.json", "summary.json", "results.json"):
        data = (directory / name).read_bytes()
        if manifest.get(name) != sha(data):
            raise ValueError(f"prior yank evidence checksum mismatch: {name}")
        records[name] = json.loads(data)
        hashes[name] = sha(data)
    prior = records["input.json"]
    if prior["source_files_sha256"] != scope["source_files_sha256"] or prior["selected_packages"] != scope["selected_packages"]:
        raise ValueError("follow-up scope differs from the frozen audit input")
    summary = records["summary.json"]
    coverage_start = summary.get("coverage_started_at_utc", summary["started_at_utc"])
    age = (datetime.datetime.now(datetime.timezone.utc) - datetime.datetime.fromisoformat(coverage_start)).total_seconds()
    if not 0 <= age <= 900:
        raise ValueError("follow-up observations exceed 15-minute window; start a new full run")
    wanted = {(item["name"], item["version"]): item for item in scope["selected_packages"]}
    seen, retained, missing = set(), [], []
    for item in records["results.json"]:
        key = (item["name"], item["version"])
        if key not in wanted or key in seen or item["targets"] != wanted[key]["targets"]:
            raise ValueError("prior yank result identity is inconsistent")
        seen.add(key)
        if item["status"] == "unavailable":
            missing.append(wanted[key])
        elif item["status"] in ("not_yanked", "yanked", "checksum_mismatch"):
            if item["locked_checksum"] != wanted[key]["checksum"]:
                raise ValueError("prior result locked checksum differs")
            retained.append(item)
        else:
            raise ValueError("unknown prior result status")
    if seen != set(wanted) or not missing:
        raise ValueError("prior run must cover the same scope and contain unavailable observations")
    return missing, retained, {"directory": str(directory), "files_sha256": hashes,
                               "prior_summary": summary, "retained_observation_count": len(retained),
                               "coverage_started_at_utc": coverage_start,
                               "meaning": "Composite follow-up: only unavailable observations are fetched again; all prior findings remain."}


def check(args):
    scope, lock = load_scope(args.audit_run.resolve())
    selected, retained, follow_up = scope["selected_packages"], [], None
    if args.follow_up:
        selected, retained, follow_up = follow_up_scope(args.follow_up.resolve(), scope)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "responses").mkdir()
    (output / "Cargo.lock.snapshot").write_bytes(lock)
    grouped = {}
    for package in selected:
        grouped.setdefault(package["name"], []).append(package)
    if len(grouped) > 1000:
        raise ValueError("unique crate count exceeds request bound")
    scope.update(started_at_utc=now(), script_sha256=sha(Path(__file__).read_bytes()), request_headers=HEADERS, follow_up=follow_up,
                 index_url=INDEX, unique_crate_count=len(grouped), planned_request_count=len(grouped) + 1,
                 limits={"workers": args.workers, "minimum_request_spacing_seconds": REQUEST_INTERVAL,
                         "socket_timeout_seconds": SOCKET_TIMEOUT, "run_budget_seconds": MAX_SECONDS,
                         "per_response_max_bytes": MAX_RESPONSE, "total_body_max_bytes": MAX_TOTAL, "retries": 0})
    write_json(output / "input.json", scope)
    limits = Limits()
    config, config_body = fetch("registry-config", "config.json", output, limits)
    try:
        configuration = json.loads(config_body.decode("utf-8", errors="strict")) if config_body is not None else None
        if not isinstance(configuration, dict) or configuration.get("auth-required", False) is not False:
            raise ValueError("registry configuration unavailable or requires authentication")
    except (ValueError, TypeError):
        limits.stop("registry configuration unavailable or requires authentication")
    results, requests = list(retained), [config]

    def observe(name):
        observation, body = fetch(name, index_path(name), output, limits)
        try:
            if body is None:
                raise ValueError(observation.get("reason", "HTTP observation unavailable"))
            entries = evaluate(name, grouped[name], body)
        except (ValueError, TypeError) as error:
            entries = [{**package, "status": "unavailable", "reason": str(error)} for package in grouped[name]]
        return observation, entries

    print(f"Observing {len(selected)} locked versions from {len(grouped)} crates; retaining {len(retained)} prior observations; at most {args.workers} in flight, 1.05 s spacing.", flush=True)
    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        pending = [executor.submit(observe, name) for name in sorted(grouped)]
        for number, future in enumerate(as_completed(pending), 1):
            observation, entries = future.result()
            requests.append(observation)
            results.extend(entries)
            if number % 25 == 0 or number == len(grouped):
                print(f"Observed {number}/{len(grouped)} crate requests; received {limits.received} bytes.", flush=True)
    results.sort(key=lambda item: (item["name"], item["version"]))
    summary = summarize(results, scope["advisory_summary"]["audit_exit_code"])
    summary.update(started_at_utc=scope["started_at_utc"], ended_at_utc=now(), lockfile_sha256=scope["lockfile_sha256"],
                   coverage_started_at_utc=follow_up["coverage_started_at_utc"] if follow_up else scope["started_at_utc"],
                   registry_versions=len(results), current_run_versions=len(selected), retained_prior_versions=len(retained),
                   unique_crates=len(grouped), received_body_bytes=limits.received,
                   attempted_requests=sum(request["http_status"] is not None or "body_file" in request for request in requests),
                   server_backoff_or_budget_stop=limits.stopped,
                   limitations=["Fresh network observations of the official sparse index; CDN/proxy headers are retained. No local Cargo index was used.",
                                "Crate observations span an interval, not an atomic registry snapshot; yank state may change later.",
                                "Scope is the frozen macOS/Windows app/HAL normal+build graphs, including host build dependencies, not every root lockfile package.",
                                "Socket/read budgets do not control operating-system DNS resolution time. A CI process timeout should additionally bound wall time."])
    write_json(output / "requests.json", sorted(requests, key=lambda item: item["name"]))
    write_json(output / "results.json", results)
    write_json(output / "summary.json", summary)
    write_json(output / "SHA256SUMS.json", {str(path.relative_to(output)): sha(path.read_bytes()) for path in sorted(output.rglob("*")) if path.is_file()})
    print(json.dumps(summary, indent=2), flush=True)
    return summary["combined_exit_code"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audit-run", required=True, type=Path, help="completed scripts/audit-dependencies.py evidence directory")
    parser.add_argument("--output", required=True, type=Path, help="new evidence directory; no overwriting or reuse of network responses")
    parser.add_argument("--workers", type=int, choices=(1, 2, 3, 4), default=4)
    parser.add_argument("--follow-up", type=Path, help="prior incomplete run; begin within 15 minutes of its earliest observation, fetch only unavailable items, and preserve prior results")
    args = parser.parse_args()
    try:
        return check(args)
    except (OSError, ValueError, TypeError, KeyError, RuntimeError) as error:
        print(f"Fresh yank verification unavailable: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
