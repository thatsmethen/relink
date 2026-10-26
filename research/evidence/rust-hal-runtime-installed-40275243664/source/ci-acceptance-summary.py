#!/usr/bin/env python3
"""Read-only, bounded console fallback for original HAL/runtime CI reports.

Never loads PCM, runs an analyzer, writes evidence, or determines acceptance.
Only fixed filenames/keys and known enums are printed. Free text is withheld.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import stat

ROOT = Path(__file__).resolve().parents[1]
MAX_READ = 128 * 1024  # Per report; JSONL samples half from each end if larger.
MAX_LINE = 8192
MAX_OUTPUT = 48 * 1024
PREFIX = 'CI_ACCEPTANCE_SUMMARY '
COMMON = ('custody-manifest.json', 'harness-error.json', 'run-error.json',
          'cleanup.json', 'cleanup-error.json', 'manifest-error.json')
FILES = {
    'hal': COMMON + ('acceptance.json', 'tone-writer.json', 'reader-a.json', 'reader-b.json',
                    'restarted-silence.json', 'tone-writer-analysis.json', 'reader-a-analysis.json',
                    'reader-b-analysis.json', 'independent-readers.json', 'restarted-silence-analysis.json'),
    'runtime': COMMON + ('runtime-result.json', 'runtime-native-timing.jsonl', 'runtime-acceptance.json',
                        'runtime.ready.json', 'source-writer.json', 'runtime-reader.json',
                        'source-writer-analysis.json', 'runtime-output-analysis.json'),
}
KEYS = frozenset('''schema pass ready complete fault running status stop_status destroy_status matches
frames events target_frames sample_rate channels captured_frames rendered_frames buffered_frames
callback_count applied_control_version clock_correction_ppm attempt processes observations
exit_code timed_out deadline_seconds completion_observed_seconds supervision_finished_seconds
term_sent kill_sent name mode clock sample_gaps host_reversals measured_sample_rate after_sample
delta_frames peak longest_stereo_zero_run interior_frames overlap_frames maximum_difference
differing_samples_above_1e-6 relative_start_frames phases phase acknowledgement_seconds
detected_response_seconds stopped_tail_peak stop_response_seconds observed_rendered_frames_per_second
unmuted_zero_runs begin_frame end_frame native_output_shutdown_proven native_output_shutdown_observed
native_shutdown_observed observed_host_time first_fault output_timing captures native_timing
metadata_pending metadata origin direction endpoint_index epoch observed_mach_host_time native_object
packet_index property selector scope element previous current reason sample_time sample_time_bits
host_time flags rate_scalar rate_scalar_bits queue_frame callback_index entry_host_time now presentation
first_fault_pending packets_seen callbacks_seen progress_count last_packet_contended
callback_completion_contended completion_contended observation observer_diagnostics_enabled
'''.split())
ENUMS = frozenset('''tone-writer reader-a reader-b restarted-silence source-writer runtime-reader runtime
play record silence capture-mix running stopped shutdown coreaudio Runtime Input Output
NativeInputPacket NativeInputReentry NativeInputProperty NativeInputOverload NativeInputLifecycle
CpalOutputError CpalCoreAudioOutputOverload CaptureTransportProducer CaptureTransportConsumer
CaptureStartup OutputBuffer MixerControls MixerRender WasapiInput CpalInputError NativeOutputPacket
NativeOutputReentry NativeOutputProperty NativeOutputOverload NativeOutputLifecycle NativeOutputStartup
WasapiOutput NativeOutputProgress OutputFanoutProducer OutputFanoutConsumer OutputFanoutStartup
InvalidBuffer InvalidTimestamp SampleDiscontinuity HostDiscontinuity EpochChanged Reentry CounterExhausted
'''.split())
TEXT_KEYS = frozenset(('error', 'observer_error', 'supervision_error', 'exceptions', 'failures', 'skipped'))
SAFE_MESSAGES = frozenset((
    'runtime did not publish real capture/mix readiness',
    'runtime readiness lacked real captured/rendered frames',
    'independent recorder clock/continuity failed', 'runtime reported failure',
    'independent recorder reported incomplete/faulted execution',
    'sample clock or callback continuity failed', 'frame/event counts do not match',
    'PCM/event/frame custody mismatch', 'missing or repeated runtime protocol events',
    'runtime telemetry reported failure or was absent', 'runtime telemetry regressed',
    'interior contains an exact-zero hole of at least eight stereo frames',
    'unmuted output contains a silent packet, including the live gain handoff',
    'stop did not produce and preserve silence within 250 ms',
    'stopped/restarted driver exposed stale audio', 'No installation ownership was recorded',
))


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def text_summary(value, depth=0):
    if depth > 8:
        return {'depth_truncated': True}
    if value is None:
        return None
    if isinstance(value, list):
        return {'count': len(value), 'items': [text_summary(v, depth + 1) for v in value[:4]],
                'omitted': max(0, len(value) - 4)}
    if isinstance(value, str):
        if value in SAFE_MESSAGES:
            return {'known_message': value}
        return {'text_withheld': True, 'characters': len(value), 'sha256': sha(value.encode('utf-8', errors='surrogatepass'))}
    return {'invalid_type': True}


def project(value, depth=0):
    if depth > 8:
        return {'depth_truncated': True}
    if value is None or type(value) is bool:
        return value
    if type(value) is int:
        return value if -(2**63) <= value <= 2**64 - 1 else {'number_out_of_range': True}
    if type(value) is float:
        return value if math.isfinite(value) and abs(value) <= 1e30 else {'number_out_of_range': True}
    if isinstance(value, str):
        return value if value in ENUMS else {'text_withheld': True}
    if isinstance(value, list):
        return {'count': len(value), 'items': [project(v, depth + 1) for v in value[:4]],
                'omitted': max(0, len(value) - 4)}
    if isinstance(value, dict):
        result = {key: project(value[key], depth + 1) for key in sorted(KEYS & value.keys())}
        result.update({key: text_summary(value[key]) for key in sorted(TEXT_KEYS & value.keys())})
        if isinstance(value.get('devices'), dict):
            result['devices'] = {f'cable{index}': project(value['devices'][uid], depth + 1)
                                 for index, uid in enumerate(('au.textual.ravelink.cable.1', 'au.textual.ravelink.cable.2'), 1)
                                 if uid in value['devices']}
        result['unlisted_fields_omitted'] = len(value.keys() - KEYS - TEXT_KEYS - {'devices'})
        return result
    return {'invalid_type': True}


def parse(raw):
    # Bound nesting before invoking the JSON decoder (including ignored fields).
    # Braces inside JSON strings do not contribute to container depth.
    depth, quoted, escaped = 0, False, False
    for character in raw.decode('utf-8'):
        if quoted:
            if escaped:
                escaped = False
            elif character == '\\':
                escaped = True
            elif character == '"':
                quoted = False
        elif character == '"':
            quoted = True
        elif character in '[{':
            depth += 1
            if depth > 32:
                raise ValueError('excessive JSON nesting')
        elif character in ']}':
            depth -= 1
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError('duplicate key')
            result[key] = value
        return result
    def constant(_):
        raise ValueError('nonfinite constant')
    value = json.loads(raw, object_pairs_hook=pairs, parse_constant=constant)
    if not isinstance(value, dict):
        raise ValueError('expected object')
    return value


def manifest_summary(data, kind):
    result = identity(data)
    records = data.get('files')
    if not isinstance(records, list):
        return dict(result, files_missing_or_invalid=True)
    result['entry_count'] = len(records)
    # Never print arbitrary manifest paths or follow them. Hashes of these fixed
    # report/PCM names are metadata only; the PCM files themselves are not opened.
    selected = set(FILES[kind]) | {'runtime-reader.wav', 'reader-a.wav', 'reader-b.wav', 'restarted-silence.wav'}
    entries = []
    for record in records:
        if not isinstance(record, dict) or record.get('path') not in selected:
            continue
        digest, size = record.get('sha256'), record.get('bytes')
        valid = isinstance(digest, str) and re.fullmatch(r'[0-9a-f]{64}', digest) and type(size) is int and 0 <= size < 2**64
        entries.append({'file': record['path'], 'sha256': digest, 'bytes': size} if valid
                       else {'file': record['path'], 'metadata_invalid': True})
    result['selected_entries'] = sorted(entries, key=lambda entry: entry['file'])[:24]
    result['selected_entries_omitted'] = max(0, len(entries) - 24)
    result['unlisted_entries_omitted'] = len(records) - len(entries)
    result['hashes_are_manifest_claims_not_reverified_payloads'] = True
    return result


def identity(data):
    result = {}
    for key, pattern in (('source_revision', r'[0-9a-f]{40}'), ('run_id', r'[0-9]{1,20}'), ('run_attempt', r'[0-9]{1,10}')):
        value = data.get(key)
        result[key] = value if isinstance(value, str) and re.fullmatch(pattern, value) else {'missing_or_invalid': True}
    return result


def read_record(directory_fd, name, kind):
    result = {'file': name}
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW, dir_fd=directory_fd)
    except FileNotFoundError:
        return dict(result, status='missing')
    except OSError as error:
        return dict(result, status='unreadable', errno=error.errno)
    try:
        with os.fdopen(fd, 'rb') as stream:
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode):
                return dict(result, status='not_regular_file')
            size = before.st_size; result['bytes'] = size
            if name.endswith('.jsonl') and size > MAX_READ:
                half = MAX_READ // 2
                head = stream.read(half); stream.seek(size - half); tail = stream.read(half)
                chunks = [(0, head[:head.rfind(b'\n') + 1]), (size - half, tail[tail.find(b'\n') + 1:])]
                result.update(status='partial', input_truncated=True, sampled_ranges='bounded_head_and_tail',
                              sampled_bytes=len(head) + len(tail), sampled_sha256=sha(head + tail))
            else:
                raw = stream.read(MAX_READ + 1)
                if len(raw) > MAX_READ:
                    return dict(result, status='input_truncated', limit_bytes=MAX_READ,
                                prefix_sha256=sha(raw[:MAX_READ]))
                chunks = [(0, raw)]; result.update(status='read', sha256=sha(raw))
            after = os.fstat(stream.fileno())
            if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
                return dict(result, status='changed_during_read')
    except OSError as error:
        return dict(result, status='unreadable', errno=error.errno)
    try:
        if name.endswith('.jsonl'):
            records, invalid = [], 0
            for _, raw in chunks:
                for line in raw.splitlines():
                    if not line.strip():
                        continue
                    try:
                        records.append(parse(line))
                    except (ValueError, UnicodeError, RecursionError):
                        invalid += 1
            first_fault = next((r for r in records if r.get('fault') not in (None, 0) or r.get('first_fault') is not None), None)
            result.update(sampled_records=len(records), malformed_records=invalid,
                          first_sampled_record=project(records[0]) if records else None,
                          first_sampled_fault=project(first_fault) if first_fault else None,
                          last_sampled_record=project(records[-1]) if records else None,
                          unsampled_fault_absence_not_established=True)
            if invalid:
                result['status'] = 'partial_malformed'
        else:
            data = parse(chunks[0][1])
            result['summary'] = manifest_summary(data, kind) if name == 'custody-manifest.json' else project(data)
    except (ValueError, UnicodeError, RecursionError, TypeError):
        result['status'] = 'malformed_json'
    return result


def collect(directory: Path, kind: str):
    # The CLI chooses a fixed repository path; no directory scans or arbitrary
    # path option. Reject symlinked ancestors and entries before reading.
    try:
        if directory.absolute() != directory.resolve(strict=False):
            raise ValueError('symlinked directory')
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except (OSError, ValueError, RuntimeError) as error:
        status = 'missing' if isinstance(error, FileNotFoundError) else 'unreadable_or_symlinked'
        return [{'file': name, 'status': status, 'directory_unavailable': True} for name in FILES[kind]]
    try:
        return [read_record(fd, name, kind) for name in FILES[kind]]
    finally:
        os.close(fd)


def render(records, kind):
    def line(value):
        return PREFIX + json.dumps(value, sort_keys=True, ensure_ascii=True, separators=(',', ':'), allow_nan=False) + '\n'
    lines = [line({'schema': 1, 'kind': kind, 'role': 'diagnostic_only_not_acceptance',
                   'read_limit_per_file': MAX_READ, 'output_limit_bytes': MAX_OUTPUT,
                   'runner_identity': identity({'source_revision': os.environ.get('GITHUB_SHA'),
                                                'run_id': os.environ.get('GITHUB_RUN_ID'),
                                                'run_attempt': os.environ.get('GITHUB_RUN_ATTEMPT')})})]
    for record in records:
        encoded = line(record)
        # Reserve a small explicit status line for every remaining file.
        remaining = len(records) - (len(lines) - 1)
        if len(encoded.encode()) > MAX_LINE or sum(len(item.encode()) for item in lines) + len(encoded.encode()) + remaining * 1024 > MAX_OUTPUT:
            abbreviated = {key: record[key] for key in ('file', 'status', 'bytes', 'sha256') if key in record}
            summary = record.get('summary', {})
            if type(summary.get('pass')) is bool:
                abbreviated['reported_pass'] = summary['pass']
            fault = record.get('first_sampled_fault')
            if isinstance(fault, dict):
                abbreviated['sampled_fault'] = {key: fault[key] for key in ('fault', 'running', 'observed_host_time')
                                               if key in fault and type(fault[key]) in (int, bool)}
                fault_identity = fault.get('first_fault')
                if isinstance(fault_identity, dict):
                    abbreviated['fault_identity'] = {key: fault_identity[key] for key in ('fault', 'origin', 'direction', 'endpoint_index', 'metadata_pending')
                                                     if key in fault_identity and type(fault_identity[key]) in (int, bool, str)}
            encoded = line(abbreviated | {'summary_truncated': True, 'summary_bytes': len(encoded.encode()),
                                         'summary_sha256': sha(encoded.encode())})
        lines.append(encoded)
    return ''.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kind', choices=tuple(FILES), required=True)
    args = parser.parse_args()
    directory = ROOT / 'build' / ('rust-hal-system' if args.kind == 'hal' else 'rust-runtime-system')
    print(render(collect(directory, args.kind), args.kind), end='')
    # Missing/failed reports are observations, never a replacement job result.
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
