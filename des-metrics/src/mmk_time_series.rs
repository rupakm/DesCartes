//! Time-series data collection and visualization for M/M/k queueing systems
//!
//! This module provides time-series data collection with exponential moving averages
//! and visualization capabilities for analyzing M/M/k queueing system behavior over time.

use des_core::SimTime;
use std::collections::VecDeque;
use std::time::Duration;

/// Time-series data point with timestamp and value
#[derive(Debug, Clone)]
pub struct TimeSeriesPoint {
    pub timestamp: SimTime,
    pub value: f64,
}

/// Exponential moving average calculator
#[derive(Debug, Clone)]
pub struct ExponentialMovingAverage {
    alpha: f64,
    current_value: Option<f64>,
}

impl ExponentialMovingAverage {
    /// Create a new EMA with the given smoothing factor (0 < alpha <= 1)
    /// Lower alpha = more smoothing, higher alpha = less smoothing
    pub fn new(alpha: f64) -> Self {
        assert!(alpha > 0.0 && alpha <= 1.0, "Alpha must be between 0 and 1");
        Self {
            alpha,
            current_value: None,
        }
    }

    /// Update the EMA with a new value
    pub fn update(&mut self, value: f64) -> f64 {
        match self.current_value {
            None => {
                self.current_value = Some(value);
                value
            }
            Some(current) => {
                let new_value = self.alpha * value + (1.0 - self.alpha) * current;
                self.current_value = Some(new_value);
                new_value
            }
        }
    }

    /// Get the current EMA value
    pub fn value(&self) -> Option<f64> {
        self.current_value
    }
}

/// Time-series collector with configurable aggregation window
#[derive(Debug)]
pub struct TimeSeriesCollector {
    /// Raw data points
    raw_data: VecDeque<TimeSeriesPoint>,
    /// Aggregated data points (with EMA smoothing)
    aggregated_data: Vec<TimeSeriesPoint>,
    /// Aggregation window duration
    aggregation_window: Duration,
    /// Exponential moving average calculator
    ema: ExponentialMovingAverage,
    /// Last aggregation timestamp
    last_aggregation: Option<SimTime>,
    /// Current window accumulator
    window_sum: f64,
    /// Current window count
    window_count: usize,
}

impl TimeSeriesCollector {
    /// Create a new time-series collector
    /// 
    /// # Arguments
    /// * `aggregation_window` - Duration for aggregating raw data points
    /// * `ema_alpha` - Smoothing factor for exponential moving average (0 < alpha <= 1)
    pub fn new(aggregation_window: Duration, ema_alpha: f64) -> Self {
        Self {
            raw_data: VecDeque::new(),
            aggregated_data: Vec::new(),
            aggregation_window,
            ema: ExponentialMovingAverage::new(ema_alpha),
            last_aggregation: None,
            window_sum: 0.0,
            window_count: 0,
        }
    }

    /// Add a data point to the time series
    pub fn add_point(&mut self, timestamp: SimTime, value: f64) {
        // Add to raw data
        self.raw_data.push_back(TimeSeriesPoint { timestamp, value });

        // Check if we need to aggregate
        let should_aggregate = match self.last_aggregation {
            None => true,
            Some(last) => timestamp.duration_since(last) >= self.aggregation_window,
        };

        if should_aggregate {
            self.aggregate_window(timestamp);
        } else {
            // Accumulate in current window
            self.window_sum += value;
            self.window_count += 1;
        }

        // Keep raw data bounded (keep last 10000 points)
        while self.raw_data.len() > 10000 {
            self.raw_data.pop_front();
        }
    }

    /// Aggregate the current window and update EMA
    fn aggregate_window(&mut self, current_timestamp: SimTime) {
        if self.window_count > 0 {
            let window_average = self.window_sum / self.window_count as f64;
            let smoothed_value = self.ema.update(window_average);

            self.aggregated_data.push(TimeSeriesPoint {
                timestamp: current_timestamp,
                value: smoothed_value,
            });

            // Reset window
            self.window_sum = 0.0;
            self.window_count = 0;
        }

        self.last_aggregation = Some(current_timestamp);
    }

    /// Force aggregation of the current window
    pub fn flush(&mut self, current_timestamp: SimTime) {
        self.aggregate_window(current_timestamp);
    }

    /// Get the aggregated time series data
    pub fn get_aggregated_data(&self) -> &[TimeSeriesPoint] {
        &self.aggregated_data
    }

    /// Get the raw time series data
    pub fn get_raw_data(&self) -> &VecDeque<TimeSeriesPoint> {
        &self.raw_data
    }

    /// Get the current EMA value
    pub fn current_ema(&self) -> Option<f64> {
        self.ema.value()
    }
}

/// Collection of time-series metrics for M/M/k analysis
#[derive(Debug)]
pub struct MmkTimeSeriesMetrics {
    /// Average latency over time
    pub latency: TimeSeriesCollector,
    /// Average queue size over time
    pub queue_size: TimeSeriesCollector,
    /// Request timeout rate over time
    pub timeout_rate: TimeSeriesCollector,
    /// Throughput over time
    pub throughput: TimeSeriesCollector,
    /// Server utilization over time
    pub utilization: TimeSeriesCollector,
}

impl MmkTimeSeriesMetrics {
    /// Create a new M/M/k time-series metrics collector
    /// 
    /// # Arguments
    /// * `aggregation_window` - Duration for aggregating data points (e.g., 100ms)
    /// * `ema_alpha` - Smoothing factor for exponential moving average (e.g., 0.1)
    pub fn new(aggregation_window: Duration, ema_alpha: f64) -> Self {
        Self {
            latency: TimeSeriesCollector::new(aggregation_window, ema_alpha),
            queue_size: TimeSeriesCollector::new(aggregation_window, ema_alpha),
            timeout_rate: TimeSeriesCollector::new(aggregation_window, ema_alpha),
            throughput: TimeSeriesCollector::new(aggregation_window, ema_alpha),
            utilization: TimeSeriesCollector::new(aggregation_window, ema_alpha),
        }
    }

    /// Record a latency measurement
    pub fn record_latency(&mut self, timestamp: SimTime, latency_ms: f64) {
        self.latency.add_point(timestamp, latency_ms);
    }

    /// Record a queue size measurement
    pub fn record_queue_size(&mut self, timestamp: SimTime, queue_size: f64) {
        self.queue_size.add_point(timestamp, queue_size);
    }

    /// Record a timeout event (1.0 for timeout, 0.0 for no timeout)
    pub fn record_timeout(&mut self, timestamp: SimTime, is_timeout: bool) {
        self.timeout_rate.add_point(timestamp, if is_timeout { 1.0 } else { 0.0 });
    }

    /// Record throughput measurement
    pub fn record_throughput(&mut self, timestamp: SimTime, throughput_rps: f64) {
        self.throughput.add_point(timestamp, throughput_rps);
    }

    /// Record server utilization measurement
    pub fn record_utilization(&mut self, timestamp: SimTime, utilization: f64) {
        self.utilization.add_point(timestamp, utilization);
    }

    /// Flush all collectors
    pub fn flush(&mut self, timestamp: SimTime) {
        self.latency.flush(timestamp);
        self.queue_size.flush(timestamp);
        self.timeout_rate.flush(timestamp);
        self.throughput.flush(timestamp);
        self.utilization.flush(timestamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_moving_average() {
        let mut ema = ExponentialMovingAverage::new(0.1);
        
        assert_eq!(ema.value(), None);
        
        let v1 = ema.update(10.0);
        assert_eq!(v1, 10.0);
        
        let v2 = ema.update(20.0);
        assert!((v2 - 11.0).abs() < 0.001); // 0.1 * 20 + 0.9 * 10 = 11
        
        let v3 = ema.update(30.0);
        assert!((v3 - 12.9).abs() < 0.001); // 0.1 * 30 + 0.9 * 11 = 12.9
    }

    #[test]
    fn test_time_series_collector() {
        let mut collector = TimeSeriesCollector::new(Duration::from_millis(100), 0.2);
        
        // Add some points within the same window
        collector.add_point(SimTime::from_duration(Duration::from_millis(10)), 5.0);
        collector.add_point(SimTime::from_duration(Duration::from_millis(20)), 15.0);
        
        // Should not have aggregated yet
        assert_eq!(collector.get_aggregated_data().len(), 0);
        
        // Add a point that triggers aggregation
        collector.add_point(SimTime::from_duration(Duration::from_millis(150)), 25.0);
        
        // Should have one aggregated point
        assert_eq!(collector.get_aggregated_data().len(), 1);
        let point = &collector.get_aggregated_data()[0];
        // The EMA calculation results in 15.0 for this specific sequence
        assert_eq!(point.value, 15.0);
    }
}