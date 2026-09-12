#!/usr/bin/env python3
"""Summarize one or more receiver JSONL runs; never infer cross-host one-way latency."""
import argparse
import json
from pathlib import Path

def summarize(path):
    first = last = None
    arrivals = []
    sessions = {}
    with path.open() as f:
        for line in f:
            row = json.loads(line)
            event = row.get('event')
            if event == 'receiver_start': first = row
            if event in ('receiver_summary', 'receiver_final'): last = row
            if event == 'receive':
                seq = row['sequence']
                seen = sessions.setdefault(row['session_id'], set())
                seen.add(seq)
                arrivals.append(row)
    if not first or not last:
        raise ValueError(f'{path}: no complete startup/summary records')
    p = last['playout']
    gaps = sum(max(s) - min(s) + 1 - len(s) for s in sessions.values() if s)
    worst = sorted(arrivals, key=lambda x: x.get('arrival_excess_ms', 0), reverse=True)[:3]
    return {
        'file': str(path), 'seconds': round(last['elapsed_seconds'], 3),
        'target_buffer_ms': first['target_buffer_ms'], 'av_sync_delay_ms': first['av_sync_delay_ms'],
        'device': first['device'], 'received_packets': last['received_packets'],
        'invalid_packets': last['invalid_packets'], 'queue_dropped': last['queue_dropped'],
        'telemetry_dropped': last['telemetry_dropped'],
        'sequence_holes_in_observed_span': gaps if sessions else None,
        'late_packets': p['late_packets'], 'missing_frames': p['missing_frames'],
        'missing_output_ms': p['missing_frames'] / first['device']['sample_rate'] * 1000,
        'underrun_callbacks': p['underruns'],
        'fill_ms_min_max_final': [p['min_fill_ms'], p['max_fill_ms'], p['fill_ms']],
        'receive_to_render_mean_ms': p.get('receive_to_render_sum_ms', 0) / p['received_frame_observations'] if p.get('received_frame_observations') else None,
        'presentation_lead_ms': last.get('presentation_lead_ms'),
        'drift_ppm': p['drift_ppm'], 'asrc_ratio': p['asrc_ratio'],
        'callback_discontinuities': p.get('callback_discontinuities'),
        'arrival_interval_ms': last['arrival_interval_ms'],
        'sender_send_interval_ms': last.get('sender_send_interval_ms'),
        'sender_capture_to_send_ms': last['sender_capture_to_send_ms'],
        'kernel_queue_ms': last.get('kernel_queue_ms'),
        'callback_interval_ms': last['callback_interval_ms'],
        'callback_work_ms': last['callback_work_ms'],
        'worst_arrival_excess_events': worst,
        'one_way_latency_ms': None,
        'note': 'Sequence holes require --events and complete telemetry; first/last unseen packets are unknowable. Receive-to-render is per sample and excludes the CoreAudio presentation lead. No physical DAC or end-to-end latency measurement.'
    }

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('logs', nargs='+', type=Path)
    args = parser.parse_args()
    print(json.dumps([summarize(p) for p in args.logs], ensure_ascii=False, indent=2))
