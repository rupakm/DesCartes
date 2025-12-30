#!/usr/bin/env python3
"""
DesCartes Metrics Visualization Script

This script provides visualization and analysis functions for metrics
exported from DesCartes simulations.

Usage:
    python visualize.py path/to/metrics.json
    python visualize.py path/to/metrics_counters.csv --format csv

For interactive use:
    from visualize import load_metrics, plot_latency_distribution
    metrics = load_metrics('metrics.json')
    plot_latency_distribution(metrics)
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Dict, List, Optional

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
import seaborn as sns

# Set plotting style
sns.set_theme(style="whitegrid")
plt.rcParams['figure.figsize'] = (12, 6)


def load_json_metrics(filepath: str) -> Dict:
    """Load metrics from JSON export."""
    with open(filepath, 'r') as f:
        return json.load(f)


def load_csv_metrics(base_path: str) -> Dict:
    """Load metrics from CSV exports."""
    base = Path(base_path)
    parent = base.parent
    stem = base.stem

    metrics = {}

    # Load counters
    counters_path = parent / f"{stem}_counters.csv"
    if counters_path.exists():
        metrics['counters'] = pd.read_csv(counters_path)

    # Load gauges
    gauges_path = parent / f"{stem}_gauges.csv"
    if gauges_path.exists():
        metrics['gauges'] = pd.read_csv(gauges_path)

    # Load histograms
    histograms_path = parent / f"{stem}_histograms.csv"
    if histograms_path.exists():
        metrics['histograms'] = pd.read_csv(histograms_path)

    # Load request stats
    request_stats_path = parent / f"{stem}_request_stats.csv"
    if request_stats_path.exists():
        metrics['request_stats'] = pd.read_csv(request_stats_path)

    return metrics


def load_metrics(filepath: str, format: str = 'auto') -> Dict:
    """
    Load metrics from file.

    Args:
        filepath: Path to metrics file
        format: 'json', 'csv', or 'auto' (detect from extension)

    Returns:
        Dictionary containing metrics data
    """
    path = Path(filepath)

    if format == 'auto':
        format = 'json' if path.suffix == '.json' else 'csv'

    if format == 'json':
        return load_json_metrics(filepath)
    else:
        return load_csv_metrics(filepath)


def plot_latency_distribution(metrics: Dict, output_path: Optional[str] = None):
    """Plot latency distribution with percentiles."""
    if 'snapshot' in metrics:
        # JSON format
        histograms = pd.DataFrame(metrics['snapshot']['histograms'])
    else:
        # CSV format
        histograms = metrics.get('histograms')

    if histograms is None or histograms.empty:
        print("No histogram data available")
        return

    fig, ax = plt.subplots(figsize=(14, 6))

    # Plot percentile bars
    x_labels = []
    means = []
    p50s = []
    p95s = []
    p99s = []

    for idx, row in histograms.iterrows():
        if 'labels' in row and isinstance(row['labels'], str) and row['labels']:
            label = f"{row['name']}:{row['labels']}"
        else:
            label = row['name']

        x_labels.append(label)
        if 'stats' in row:
            stats = row['stats']
            means.append(stats['mean'])
            p50s.append(stats['median'])
            p95s.append(stats['p95'])
            p99s.append(stats['p99'])
        else:
            means.append(row.get('mean', 0))
            p50s.append(row.get('median', 0))
            p95s.append(row.get('p95', 0))
            p99s.append(row.get('p99', 0))

    x = np.arange(len(x_labels))
    width = 0.2

    ax.bar(x - 1.5*width, means, width, label='Mean', color='#2E86AB')
    ax.bar(x - 0.5*width, p50s, width, label='P50', color='#A23B72')
    ax.bar(x + 0.5*width, p95s, width, label='P95', color='#F18F01')
    ax.bar(x + 1.5*width, p99s, width, label='P99', color='#C73E1D')

    ax.set_xlabel('Metric', fontsize=12, fontweight='bold')
    ax.set_ylabel('Latency (ms)', fontsize=12, fontweight='bold')
    ax.set_title('Latency Distribution by Percentile', fontsize=14, fontweight='bold')
    ax.set_xticks(x)
    ax.set_xticklabels(x_labels, rotation=45, ha='right')
    ax.legend()
    ax.grid(axis='y', alpha=0.3)

    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved latency distribution plot to {output_path}")
    else:
        plt.show()


def plot_throughput(metrics: Dict, output_path: Optional[str] = None):
    """Plot throughput metrics."""
    if 'snapshot' in metrics:
        counters = pd.DataFrame(metrics['snapshot']['counters'])
    else:
        counters = metrics.get('counters')

    if counters is None or counters.empty:
        print("No counter data available")
        return

    # Sort by value and take top 15
    counters_sorted = counters.sort_values('value', ascending=False).head(15)

    fig, ax = plt.subplots(figsize=(12, 8))

    # Create labels
    labels = []
    for idx, row in counters_sorted.iterrows():
        if 'labels' in row and isinstance(row['labels'], str) and row['labels']:
            labels.append(f"{row['name']}\\n{row['labels']}")
        else:
            labels.append(row['name'])

    colors = sns.color_palette("viridis", len(counters_sorted))
    bars = ax.barh(labels, counters_sorted['value'], color=colors)

    # Add value labels on bars
    for bar in bars:
        width = bar.get_width()
        ax.text(width, bar.get_y() + bar.get_height()/2,
                f' {int(width):,}',
                ha='left', va='center', fontsize=9)

    ax.set_xlabel('Count', fontsize=12, fontweight='bold')
    ax.set_title('Throughput Metrics (Top 15)', fontsize=14, fontweight='bold')
    ax.grid(axis='x', alpha=0.3)

    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved throughput plot to {output_path}")
    else:
        plt.show()


def plot_percentile_comparison(metrics: Dict, output_path: Optional[str] = None):
    """Plot percentile comparison across metrics."""
    if 'snapshot' in metrics:
        histograms = pd.DataFrame(metrics['snapshot']['histograms'])
    else:
        histograms = metrics.get('histograms')

    if histograms is None or histograms.empty:
        print("No histogram data available")
        return

    fig, ax = plt.subplots(figsize=(12, 6))

    percentiles = ['P50', 'P95', 'P99']

    for idx, row in histograms.iterrows():
        if 'labels' in row and isinstance(row['labels'], str) and row['labels']:
            label = f"{row['name']}:{row['labels']}"
        else:
            label = row['name']

        if 'stats' in row:
            stats = row['stats']
            values = [stats['median'], stats['p95'], stats['p99']]
        else:
            values = [row.get('median', 0), row.get('p95', 0), row.get('p99', 0)]

        ax.plot([0, 1, 2], values, marker='o', linewidth=2, markersize=8, label=label)

    ax.set_xticks([0, 1, 2])
    ax.set_xticklabels(percentiles)
    ax.set_xlabel('Percentile', fontsize=12, fontweight='bold')
    ax.set_ylabel('Latency (ms)', fontsize=12, fontweight='bold')
    ax.set_title('Latency Percentiles Comparison', fontsize=14, fontweight='bold')
    ax.legend(bbox_to_anchor=(1.05, 1), loc='upper left')
    ax.grid(alpha=0.3)

    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved percentile comparison plot to {output_path}")
    else:
        plt.show()


def plot_request_stats(metrics: Dict, output_path: Optional[str] = None):
    """Plot request tracker statistics."""
    if 'request_stats' in metrics:
        stats = metrics['request_stats']
        if isinstance(stats, pd.DataFrame):
            # CSV format
            stats_dict = dict(zip(stats['metric'], stats['value']))
        else:
            stats_dict = stats
    elif 'snapshot' in metrics and 'request_stats' in metrics:
        stats_dict = metrics['request_stats']
    else:
        print("No request stats available")
        return

    # Create summary visualization
    fig, axes = plt.subplots(2, 2, figsize=(14, 10))

    # Plot 1: Success/Error rates
    ax = axes[0, 0]
    rates = {
        'Success': stats_dict.get('success_rate', 0) * 100,
        'Error': stats_dict.get('error_rate', 0) * 100,
        'Timeout': stats_dict.get('timeout_rate', 0) * 100
    }
    colors = ['#2E86AB', '#C73E1D', '#F18F01']
    ax.bar(rates.keys(), rates.values(), color=colors)
    ax.set_ylabel('Percentage', fontweight='bold')
    ax.set_title('Request Success/Error Rates', fontweight='bold')
    ax.set_ylim(0, 100)
    ax.grid(axis='y', alpha=0.3)

    # Plot 2: Throughput metrics
    ax = axes[0, 1]
    throughput = {
        'Goodput': stats_dict.get('goodput', 0),
        'Throughput': stats_dict.get('throughput', 0)
    }
    ax.bar(throughput.keys(), throughput.values(), color=['#2E86AB', '#A23B72'])
    ax.set_ylabel('Requests/sec', fontweight='bold')
    ax.set_title('Goodput vs Throughput', fontweight='bold')
    ax.grid(axis='y', alpha=0.3)

    # Plot 3: Request latency percentiles
    ax = axes[1, 0]
    req_latency = {
        'Mean': stats_dict.get('request_latency_mean_ms', 0),
        'P50': stats_dict.get('request_latency_median_ms', 0),
        'P95': stats_dict.get('request_latency_p95_ms', 0),
        'P99': stats_dict.get('request_latency_p99_ms', 0)
    }
    ax.bar(req_latency.keys(), req_latency.values(),
           color=['#2E86AB', '#A23B72', '#F18F01', '#C73E1D'])
    ax.set_ylabel('Latency (ms)', fontweight='bold')
    ax.set_title('Request Latency Distribution', fontweight='bold')
    ax.grid(axis='y', alpha=0.3)

    # Plot 4: Active vs Completed
    ax = axes[1, 1]
    counts = {
        'Active\\nRequests': stats_dict.get('active_requests', 0),
        'Completed\\nRequests': stats_dict.get('completed_requests', 0),
        'Active\\nAttempts': stats_dict.get('active_attempts', 0),
        'Completed\\nAttempts': stats_dict.get('completed_attempts', 0)
    }
    colors_counts = ['#2E86AB', '#A23B72', '#F18F01', '#C73E1D']
    ax.bar(counts.keys(), counts.values(), color=colors_counts)
    ax.set_ylabel('Count', fontweight='bold')
    ax.set_title('Request/Attempt Counts', fontweight='bold')
    ax.grid(axis='y', alpha=0.3)

    plt.suptitle('Request Tracker Statistics', fontsize=16, fontweight='bold', y=1.00)
    plt.tight_layout()

    if output_path:
        plt.savefig(output_path, dpi=300, bbox_inches='tight')
        print(f"Saved request stats plot to {output_path}")
    else:
        plt.show()


def generate_all_plots(metrics: Dict, output_dir: str = 'output'):
    """Generate all visualization plots."""
    output_path = Path(output_dir)
    output_path.mkdir(exist_ok=True)

    print(f"Generating plots in {output_dir}/...")

    plot_latency_distribution(metrics, str(output_path / 'latency_distribution.png'))
    plot_throughput(metrics, str(output_path / 'throughput.png'))
    plot_percentile_comparison(metrics, str(output_path / 'percentile_comparison.png'))
    plot_request_stats(metrics, str(output_path / 'request_stats.png'))

    print(f"\\n✓ All plots generated in {output_dir}/")


def print_summary(metrics: Dict):
    """Print summary statistics."""
    print("\\n=== Metrics Summary ===\\n")

    if 'snapshot' in metrics:
        snapshot = metrics['snapshot']
        print(f"Timestamp: {snapshot['timestamp_secs']:.3f}s")
        print(f"Counters: {len(snapshot['counters'])}")
        print(f"Gauges: {len(snapshot['gauges'])}")
        print(f"Histograms: {len(snapshot['histograms'])}")
    elif 'counters' in metrics:
        print(f"Counters: {len(metrics.get('counters', []))}")
        print(f"Gauges: {len(metrics.get('gauges', []))}")
        print(f"Histograms: {len(metrics.get('histograms', []))}")

    if 'request_stats' in metrics:
        print("\\nRequest Statistics:")
        stats = metrics['request_stats']
        if isinstance(stats, pd.DataFrame):
            for _, row in stats.head(10).iterrows():
                print(f"  {row['metric']}: {row['value']}")
        else:
            for key in ['goodput', 'throughput', 'success_rate', 'error_rate']:
                if key in stats:
                    value = stats[key]
                    if 'rate' in key:
                        print(f"  {key}: {value*100:.1f}%")
                    else:
                        print(f"  {key}: {value:.2f}")


def main():
    parser = argparse.ArgumentParser(description='Visualize DesCartes simulation metrics')
    parser.add_argument('filepath', help='Path to metrics file (JSON or CSV)')
    parser.add_argument('--format', choices=['json', 'csv', 'auto'], default='auto',
                        help='Input format (default: auto-detect)')
    parser.add_argument('--output', '-o', help='Output directory for plots', default='output')
    parser.add_argument('--summary', '-s', action='store_true',
                        help='Print summary only (no plots)')

    args = parser.parse_args()

    # Load metrics
    print(f"Loading metrics from {args.filepath}...")
    try:
        metrics = load_metrics(args.filepath, args.format)
    except Exception as e:
        print(f"Error loading metrics: {e}", file=sys.stderr)
        sys.exit(1)

    # Print summary
    print_summary(metrics)

    if not args.summary:
        # Generate plots
        generate_all_plots(metrics, args.output)
        print(f"\\nOpen the plots in {args.output}/ to view visualizations")


if __name__ == '__main__':
    main()
