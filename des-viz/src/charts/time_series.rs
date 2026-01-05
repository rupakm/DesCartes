//! Time-series chart generation for simulation metrics
//!
//! This module provides functionality to create time-series line charts
//! showing metrics evolution over time.

use crate::error::VizError;
use crate::charts::ChartConfig;
use des_core::SimTime;
use plotters::prelude::*;
use std::path::Path;

/// A single time-series data point
#[derive(Debug, Clone)]
pub struct TimeSeriesPoint {
    pub timestamp: SimTime,
    pub value: f64,
}

/// A complete time-series with metadata
#[derive(Debug, Clone)]
pub struct TimeSeries {
    pub name: String,
    pub data: Vec<TimeSeriesPoint>,
    pub color: RGBColor,
}

impl TimeSeries {
    /// Create a new time series
    pub fn new(name: impl Into<String>, data: Vec<TimeSeriesPoint>, color: RGBColor) -> Self {
        Self {
            name: name.into(),
            data,
            color,
        }
    }
}

/// Create a time-series chart with multiple series
///
/// # Arguments
/// * `series` - Vector of time series to plot
/// * `config` - Chart configuration
/// * `output_path` - Path to save the chart
///
/// # Example
/// ```no_run
/// use des_viz::charts::time_series::{TimeSeries, TimeSeriesPoint, create_time_series_chart};
/// use des_viz::charts::ChartConfig;
/// use des_core::SimTime;
/// use plotters::prelude::RED;
/// use std::time::Duration;
///
/// let data = vec![
///     TimeSeriesPoint { timestamp: SimTime::zero(), value: 10.0 },
///     TimeSeriesPoint { timestamp: SimTime::from_duration(Duration::from_secs(1)), value: 15.0 },
/// ];
/// let series = vec![TimeSeries::new("Latency", data, RED)];
/// let config = ChartConfig::new("Latency Over Time")
///     .x_label("Time (s)")
///     .y_label("Latency (ms)");
/// 
/// create_time_series_chart(&series, &config, "latency_chart.png").unwrap();
/// ```
pub fn create_time_series_chart(
    series: &[TimeSeries],
    config: &ChartConfig,
    output_path: impl AsRef<Path>,
) -> Result<(), VizError> {
    if series.is_empty() {
        return Err(VizError::InvalidData("No time series data provided".to_string()));
    }

    // Find the overall time and value ranges
    let mut min_time = SimTime::from_duration(std::time::Duration::MAX);
    let mut max_time = SimTime::zero();
    let mut min_value = f64::INFINITY;
    let mut max_value = f64::NEG_INFINITY;

    for ts in series {
        for point in &ts.data {
            if point.timestamp < min_time {
                min_time = point.timestamp;
            }
            if point.timestamp > max_time {
                max_time = point.timestamp;
            }
            if point.value < min_value {
                min_value = point.value;
            }
            if point.value > max_value {
                max_value = point.value;
            }
        }
    }

    // Handle edge cases
    if min_value == f64::INFINITY {
        min_value = 0.0;
        max_value = 1.0;
    } else if (max_value - min_value).abs() < f64::EPSILON {
        // If all values are the same, add some padding
        let padding = if min_value.abs() < f64::EPSILON { 1.0 } else { min_value.abs() * 0.1 };
        min_value -= padding;
        max_value += padding;
    }

    // Convert time range to seconds for plotting
    let time_range_secs = max_time.duration_since(min_time).as_secs_f64();
    let max_time_secs = if time_range_secs < f64::EPSILON { 1.0 } else { time_range_secs };

    // Create the chart
    let root = BitMapBackend::new(output_path.as_ref(), (config.width, config.height))
        .into_drawing_area();
    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .caption(&config.title, ("sans-serif", 40))
        .margin(20)
        .x_label_area_size(50)
        .y_label_area_size(70)
        .build_cartesian_2d(0.0..max_time_secs, min_value..max_value)?;

    chart
        .configure_mesh()
        .x_desc(&config.x_label)
        .y_desc(&config.y_label)
        .axis_desc_style(("sans-serif", 20))
        .draw()?;

    // Plot each time series
    for ts in series.iter() {
        if ts.data.is_empty() {
            continue;
        }

        // Convert data points to (time_secs, value) pairs
        let plot_data: Vec<(f64, f64)> = ts.data
            .iter()
            .map(|point| {
                let time_secs = point.timestamp.duration_since(min_time).as_secs_f64();
                (time_secs, point.value)
            })
            .collect();

        // Plot the line
        chart
            .draw_series(LineSeries::new(plot_data.iter().cloned(), &ts.color))?
            .label(&ts.name)
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 10, y)], ts.color));
    }

    // Draw the legend
    chart
        .configure_series_labels()
        .background_style(&WHITE.mix(0.8))
        .border_style(&BLACK)
        .draw()?;

    root.present()?;
    Ok(())
}

/// Create a multi-panel time-series chart for M/M/k metrics
///
/// Creates a chart with multiple subplots showing different metrics over time.
///
/// # Arguments
/// * `latency_series` - Latency time series
/// * `queue_size_series` - Queue size time series  
/// * `timeout_rate_series` - Timeout rate time series
/// * `output_path` - Path to save the chart
pub fn create_mmk_time_series_chart(
    latency_series: &TimeSeries,
    queue_size_series: &TimeSeries,
    timeout_rate_series: &TimeSeries,
    output_path: impl AsRef<Path>,
) -> Result<(), VizError> {
    let root = BitMapBackend::new(output_path.as_ref(), (1200, 900))
        .into_drawing_area();
    root.fill(&WHITE)?;

    // Split into three vertical panels
    let areas = root.split_evenly((3, 1));
    if areas.len() < 3 {
        return Err(VizError::InvalidData("Failed to create chart areas".to_string()));
    }
    
    let latency_area = &areas[0];
    let queue_area = &areas[1];
    let timeout_area = &areas[2];

    // Helper function to get time range
    let get_time_range = |series: &TimeSeries| -> (SimTime, SimTime, f64) {
        if series.data.is_empty() {
            return (SimTime::zero(), SimTime::from_duration(std::time::Duration::from_secs(1)), 1.0);
        }
        let min_time = series.data.iter().map(|p| p.timestamp).min().unwrap_or(SimTime::zero());
        let max_time = series.data.iter().map(|p| p.timestamp).max().unwrap_or(SimTime::from_duration(std::time::Duration::from_secs(1)));
        let time_range_secs = max_time.duration_since(min_time).as_secs_f64();
        (min_time, max_time, if time_range_secs < f64::EPSILON { 1.0 } else { time_range_secs })
    };

    // Helper function to get value range with padding
    let get_value_range = |series: &TimeSeries| -> (f64, f64) {
        if series.data.is_empty() {
            return (0.0, 1.0);
        }
        let min_val = series.data.iter().map(|p| p.value).fold(f64::INFINITY, f64::min);
        let max_val = series.data.iter().map(|p| p.value).fold(f64::NEG_INFINITY, f64::max);
        
        if (max_val - min_val).abs() < f64::EPSILON {
            let padding = if min_val.abs() < f64::EPSILON { 1.0 } else { min_val.abs() * 0.1 };
            (min_val - padding, max_val + padding)
        } else {
            (min_val, max_val)
        }
    };

    // Plot latency
    {
        let (min_time, _max_time, max_time_secs) = get_time_range(latency_series);
        let (min_val, max_val) = get_value_range(latency_series);

        let mut chart = ChartBuilder::on(latency_area)
            .caption("Average Latency Over Time", ("sans-serif", 30))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time_secs, min_val..max_val)?;

        chart
            .configure_mesh()
            .x_desc("Time (s)")
            .y_desc("Latency (ms)")
            .axis_desc_style(("sans-serif", 15))
            .draw()?;

        if !latency_series.data.is_empty() {
            let plot_data: Vec<(f64, f64)> = latency_series.data
                .iter()
                .map(|point| {
                    let time_secs = point.timestamp.duration_since(min_time).as_secs_f64();
                    (time_secs, point.value)
                })
                .collect();

            chart.draw_series(LineSeries::new(plot_data, &latency_series.color))?;
        }
    }

    // Plot queue size
    {
        let (min_time, _max_time, max_time_secs) = get_time_range(queue_size_series);
        let (min_val, max_val) = get_value_range(queue_size_series);

        let mut chart = ChartBuilder::on(queue_area)
            .caption("Average Queue Size Over Time", ("sans-serif", 30))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time_secs, min_val..max_val)?;

        chart
            .configure_mesh()
            .x_desc("Time (s)")
            .y_desc("Queue Size")
            .axis_desc_style(("sans-serif", 15))
            .draw()?;

        if !queue_size_series.data.is_empty() {
            let plot_data: Vec<(f64, f64)> = queue_size_series.data
                .iter()
                .map(|point| {
                    let time_secs = point.timestamp.duration_since(min_time).as_secs_f64();
                    (time_secs, point.value)
                })
                .collect();

            chart.draw_series(LineSeries::new(plot_data, &queue_size_series.color))?;
        }
    }

    // Plot timeout rate
    {
        let (min_time, _max_time, max_time_secs) = get_time_range(timeout_rate_series);
        let (min_val, max_val) = get_value_range(timeout_rate_series);

        let mut chart = ChartBuilder::on(timeout_area)
            .caption("Request Timeout Rate Over Time", ("sans-serif", 30))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0.0..max_time_secs, min_val..max_val)?;

        chart
            .configure_mesh()
            .x_desc("Time (s)")
            .y_desc("Timeout Rate")
            .axis_desc_style(("sans-serif", 15))
            .draw()?;

        if !timeout_rate_series.data.is_empty() {
            let plot_data: Vec<(f64, f64)> = timeout_rate_series.data
                .iter()
                .map(|point| {
                    let time_secs = point.timestamp.duration_since(min_time).as_secs_f64();
                    (time_secs, point.value)
                })
                .collect();

            chart.draw_series(LineSeries::new(plot_data, &timeout_rate_series.color))?;
        }
    }

    root.present()?;
    Ok(())
}