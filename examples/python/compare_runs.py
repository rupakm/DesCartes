#!/usr/bin/env python3
"""
DesCartes Simulation Comparison Tool

Compare metrics from multiple simulation runs to analyze performance differences,
regressions, or improvements.

Usage:
    python compare_runs.py baseline.json optimized.json --labels "Baseline" "Optimized"
    python compare_runs.py run1.csv run2.csv run3.csv --format csv
"""

import argparse
import sys
from pathlib import Path
from typing import Dict, List

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
import seaborn as sns
from scipy import stats

from visualize import load_metrics

# Set plotting style
sns.set_theme(style="whitegrid")


def compare_latencies(runs: List[Dict], labels: List[str], output_path: str = None):
    """Compare latency distributions across runs."""
    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(16, 6))

    # Plot 1: Mean latencies
    all_data = []
    all_labels = []

    for run, label in zip(runs, labels):
        if 'snapshot' in run:
            histograms = run['snapshot']['histograms']
        else:
            histograms = run.get('histograms', [])

        if isinstance(histograms, pd.DataFrame):
            histograms = histograms.to_dict('records')

        for hist in histograms:
            metric_name = hist['name']
            if 'stats' in hist:
                mean = hist['stats']['mean']
            else:
                mean = hist.get('mean', 0)

            all_data.append({'run': label, 'metric': metric_name, 'mean_latency': mean})

    df = pd.DataFrame(all_data)

    if not df.empty:
        # Pivot for grouped bar chart
        pivot_df = df.pivot(index='metric', columns='run', values='mean_latency')

        pivot_df.plot(kind='bar', ax=ax1, width=0.8)
        ax1.set_title('Mean Latency Comparison', fontsize=14, fontweight='bold')
        ax1.set_ylabel('Latency (ms)', fontweight='bold')
        ax1.set_xlabel('Metric', fontweight='bold')
        ax1.legend(title='Run')
        ax1.grid(axis='y', alpha=0.3)
        plt.setp(ax1.xaxis.get_majorticklabels(), rotation=45, ha='right')

    # Plot 2: P99 latencies
    all_p99_data = []

    for run, label in zip(runs, labels):
        if 'snapshot' in run:
            histograms = run['snapshot']['histograms']
        else:
            histograms = run.get('histograms', [])

        if isinstance(histograms, pd.DataFrame):
            histograms = histograms.to_dict('records')

        for hist in histograms:
            metric_name = hist['name']
            if 'stats' in hist:
                p99 = hist['stats']['p99']
            else:
                p99 = hist.get('p99', 0)

            all_p99_data.append({'run': label, 'metric': metric_name, 'p99_latency': p99})

    p99_df = pd.DataFrame(all_p99_data)

    if not p99_df.empty:
        pivot_p99_df = p99_df.pivot(index='metric', columns='run', values='p99_latency')

        pivot_p99_df.plot(kind='bar', ax=ax2, width=0.8)
        ax2.set_title('P99 Latency Comparison', fontsize=14, fontweight='bold')
        ax2.set_ylabel('Latency (ms)', fontweight='bold')
        ax2.set_xlabel('Metric', fontweight='bold')
        ax2.legend(title='Run')
        ax2.grid(axis='y', alpha=0.3)
        plt.setp(ax2.xaxis.get_majorticklabels(), rotation=45, ha='right')

    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved latency comparison to {output_path}")
    else:
        plt.show()


def compare_throughput(runs: List[Dict], labels: List[str], output_path: str = None):
    """Compare throughput metrics across runs."""
    fig, ax = plt.subplots(figsize=(12, 6))

    # Collect request stats from all runs
    data = []
    for run, label in zip(runs, labels):
        if 'request_stats' in run:
            stats = run['request_stats']
            if isinstance(stats, pd.DataFrame):
                stats_dict = dict(zip(stats['metric'], stats['value']))
            else:
                stats_dict = stats
        elif 'snapshot' in run and 'request_stats' in run:
            stats_dict = run['request_stats']
        else:
            continue

        data.append({
            'run': label,
            'Goodput': stats_dict.get('goodput', 0),
            'Throughput': stats_dict.get('throughput', 0)
        })

    df = pd.DataFrame(data)

    if not df.empty:
        x = np.arange(len(df))
        width = 0.35

        ax.bar(x - width/2, df['Goodput'], width, label='Goodput', color='#2E86AB')
        ax.bar(x + width/2, df['Throughput'], width, label='Throughput', color='#A23B72')

        ax.set_ylabel('Requests/sec', fontweight='bold')
        ax.set_title('Throughput Comparison', fontsize=14, fontweight='bold')
        ax.set_xticks(x)
        ax.set_xticklabels(df['run'])
        ax.legend()
        ax.grid(axis='y', alpha=0.3)

    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved throughput comparison to {output_path}")
    else:
        plt.show()


def compare_success_rates(runs: List[Dict], labels: List[str], output_path: str = None):
    """Compare success/error rates across runs."""
    fig, ax = plt.subplots(figsize=(12, 6))

    # Collect rate data from all runs
    data = []
    for run, label in zip(runs, labels):
        if 'request_stats' in run:
            stats = run['request_stats']
            if isinstance(stats, pd.DataFrame):
                stats_dict = dict(zip(stats['metric'], stats['value']))
            else:
                stats_dict = stats
        elif 'snapshot' in run and 'request_stats' in run:
            stats_dict = run['request_stats']
        else:
            continue

        data.append({
            'run': label,
            'Success': stats_dict.get('success_rate', 0) * 100,
            'Error': stats_dict.get('error_rate', 0) * 100,
            'Timeout': stats_dict.get('timeout_rate', 0) * 100
        })

    df = pd.DataFrame(data)

    if not df.empty:
        x = np.arange(len(df))
        width = 0.25

        ax.bar(x - width, df['Success'], width, label='Success', color='#2E86AB')
        ax.bar(x, df['Error'], width, label='Error', color='#C73E1D')
        ax.bar(x + width, df['Timeout'], width, label='Timeout', color='#F18F01')

        ax.set_ylabel('Percentage', fontweight='bold')
        ax.set_title('Success/Error Rate Comparison', fontsize=14, fontweight='bold')
        ax.set_xticks(x)
        ax.set_xticklabels(df['run'])
        ax.set_ylim(0, 100)
        ax.legend()
        ax.grid(axis='y', alpha=0.3)

    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved success rate comparison to {output_path}")
    else:
        plt.show()


def statistical_comparison(runs: List[Dict], labels: List[str]):
    """Perform statistical comparison between runs."""
    print("\\n=== Statistical Comparison ===\\n")

    if len(runs) < 2:
        print("Need at least 2 runs for comparison")
        return

    # Compare request statistics
    print("Request Statistics Comparison:")
    print("-" * 60)

    metrics_to_compare = ['goodput', 'throughput', 'success_rate', 'error_rate',
                         'request_latency_mean_ms', 'request_latency_p95_ms',
                         'request_latency_p99_ms']

    for metric in metrics_to_compare:
        values = []
        for run in runs:
            if 'request_stats' in run:
                stats = run['request_stats']
                if isinstance(stats, pd.DataFrame):
                    stats_dict = dict(zip(stats['metric'], stats['value']))
                else:
                    stats_dict = stats

                if metric in stats_dict:
                    values.append(stats_dict[metric])
                else:
                    values.append(None)

        if all(v is not None for v in values):
            print(f"\\n{metric}:")
            for label, value in zip(labels, values):
                if 'rate' in metric:
                    print(f"  {label:20s}: {value*100:6.2f}%")
                elif 'ms' in metric:
                    print(f"  {label:20s}: {value:6.2f} ms")
                else:
                    print(f"  {label:20s}: {value:6.2f}")

            # Calculate percentage difference if 2 runs
            if len(values) == 2 and values[0] != 0:
                diff_pct = ((values[1] - values[0]) / values[0]) * 100
                direction = "improvement" if ('error' in metric or 'latency' in metric) and diff_pct < 0 else \
                           "improvement" if diff_pct > 0 else "regression"
                print(f"  Difference: {diff_pct:+.1f}% ({direction})")


def generate_comparison_report(runs: List[Dict], labels: List[str], output_dir: str = 'comparison'):
    """Generate complete comparison report."""
    output_path = Path(output_dir)
    output_path.mkdir(exist_ok=True)

    print(f"\\nGenerating comparison report in {output_dir}/...\\n")

    compare_latencies(runs, labels, str(output_path / 'latency_comparison.png'))
    compare_throughput(runs, labels, str(output_path / 'throughput_comparison.png'))
    compare_success_rates(runs, labels, str(output_path / 'success_rate_comparison.png'))

    statistical_comparison(runs, labels)

    print(f"\\n✓ Comparison report generated in {output_dir}/")


def main():
    parser = argparse.ArgumentParser(description='Compare multiple DesCartes simulation runs')
    parser.add_argument('files', nargs='+', help='Metrics files to compare (JSON or CSV)')
    parser.add_argument('--labels', '-l', nargs='+', help='Labels for each run (default: Run 1, Run 2, ...)')
    parser.add_argument('--format', choices=['json', 'csv', 'auto'], default='auto',
                        help='Input format (default: auto-detect)')
    parser.add_argument('--output', '-o', help='Output directory for comparison plots',
                        default='comparison')

    args = parser.parse_args()

    if len(args.files) < 2:
        print("Error: Need at least 2 files to compare", file=sys.stderr)
        sys.exit(1)

    # Set default labels if not provided
    if args.labels:
        if len(args.labels) != len(args.files):
            print("Error: Number of labels must match number of files", file=sys.stderr)
            sys.exit(1)
        labels = args.labels
    else:
        labels = [f"Run {i+1}" for i in range(len(args.files))]

    # Load all runs
    print(f"Loading {len(args.files)} simulation runs...")
    runs = []
    for filepath, label in zip(args.files, labels):
        try:
            metrics = load_metrics(filepath, args.format)
            runs.append(metrics)
            print(f"  ✓ Loaded {label} from {filepath}")
        except Exception as e:
            print(f"Error loading {filepath}: {e}", file=sys.stderr)
            sys.exit(1)

    # Generate comparison
    generate_comparison_report(runs, labels, args.output)


if __name__ == '__main__':
    main()
