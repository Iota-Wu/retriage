#![allow(clippy::unwrap_used)]

use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main, measurement::WallTime,
};
use retriage::{
    ErrorDecision, ErrorHandler, RetryConfig, RetryConfigBuilder,
    backoff::{BoundedJitter, Exponential, FullJitter},
    retry,
};
use std::{
    error, fmt,
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

#[derive(Debug)]
enum MockError {
    Temporary,
    Permanent,
}

impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MockError::Temporary => write!(f, "temporary"),
            MockError::Permanent => write!(f, "permanent"),
        }
    }
}

impl error::Error for MockError {}

#[derive(Clone, Copy)]
struct MockPolicy;

impl ErrorHandler for MockPolicy {
    type Err = MockError;

    fn handle<'a>(
        &self,
        _e: &'a Self::Err,
        _attempt: u32,
        backoff: std::time::Duration,
    ) -> retriage::ErrorDecision<'a, Self::Err> {
        ErrorDecision::RetryAfter(backoff)
    }
}

fn bench_init<'a>(
    c: &'a mut Criterion,
    group_name: &str,
) -> (tokio::runtime::Runtime, BenchmarkGroup<'a, WallTime>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        tokio::time::pause();
    });

    let mut group = c.benchmark_group(group_name);
    group
        .sample_size(500)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3));

    (rt, group)
}

fn benches_retry_happy_path(c: &mut Criterion) {
    let (rt, mut group) = bench_init(c, "retry_with_no_error");
    let config = RetryConfigBuilder::new()
        .handler(MockPolicy)
        .max_retries(9)
        .build();

    group.bench_function("raw_async_yield", |b| {
        b.to_async(&rt).iter(|| async {
            let input = black_box(42);
            tokio::task::yield_now().await;
            black_box(Ok::<_, MockError>(black_box(input) + 1))
        })
    });

    group.bench_function("retry_no_error_with_yield", |b| {
        b.to_async(&rt).iter(|| async {
            let input = black_box(42);

            let result = retry!(
                {
                    tokio::task::yield_now().await;
                    let val = black_box(input) + 1;
                    black_box(Ok::<_, MockError>(val))
                },
                config
            )
            .await;

            black_box(result)
        })
    });

    group.finish();
}

#[inline(never)]
async fn retry_with_error<I>(
    attempts: Arc<AtomicU32>,
    target_fails: u32,
    input: i32,
    config: RetryConfig<I, MockPolicy>,
) -> Result<i32, MockError>
where
    I: Iterator<Item = Duration> + Clone,
{
    let result = retry!(
        {
            tokio::task::yield_now().await;
            let current = attempts.fetch_add(1, Ordering::Relaxed);
            if current < target_fails {
                black_box(Err::<_, MockError>(MockError::Temporary))
            } else {
                black_box(Ok::<_, MockError>(input + 1))
            }
        },
        config
    )
    .await;

    black_box(result)
}

fn benches_retry_sad_path(c: &mut Criterion) {
    let (rt, mut group) = bench_init(c, "retry_with_error");
    let config_with_max_retries_9 = RetryConfigBuilder::new()
        .handler(MockPolicy)
        .max_retries(9)
        .build();

    // Test retry behavior with transient failures
    for retry_count in [1, 3, 5] {
        group.bench_with_input(
            BenchmarkId::new("retry_transient_failures", retry_count),
            &retry_count,
            |b, &target_fails| {
                let attempts = Arc::new(AtomicU32::new(0));

                b.to_async(&rt).iter(|| {
                    let attempts = Arc::clone(&attempts);
                    attempts.store(0, Ordering::Relaxed);

                    let input = black_box(42);

                    retry_with_error(
                        Arc::clone(&attempts),
                        target_fails,
                        input,
                        config_with_max_retries_9,
                    )
                });
            },
        );
    }

    // Test retry with exhausted error
    let config_with_max_retries_3 = RetryConfigBuilder::new()
        .max_retries(3)
        .handler(MockPolicy)
        .build();

    group.bench_function("retry_exhausted_error", |b| {
        b.to_async(&rt).iter(|| async {
            let result = retry!(
                {
                    tokio::task::yield_now().await;
                    black_box(Err::<i32, MockError>(MockError::Permanent))
                },
                config_with_max_retries_3
            )
            .await;

            black_box(result)
        });
    });

    group.finish();
}

fn bench_retry_sad_path_with_jitter(c: &mut Criterion) {
    let (rt, mut group) = bench_init(c, "retry_sad_path_with_jitter");
    let full_jitter = Exponential::with_jitter(
        Duration::from_millis(100),
        Duration::from_secs(30),
        FullJitter,
    );
    let bounded_jitter = Exponential::with_jitter(
        Duration::from_millis(100),
        Duration::from_secs(30),
        BoundedJitter::new(0.2),
    );
    let config_with_full_jitter = RetryConfigBuilder::new()
        .handler(MockPolicy)
        .max_retries(9)
        .backoff(full_jitter)
        .build();
    let config_with_bounded_jitter = RetryConfigBuilder::new()
        .handler(MockPolicy)
        .max_retries(9)
        .backoff(bounded_jitter)
        .build();

    for target_fails in [1, 3] {
        group.bench_with_input(
            BenchmarkId::new("full_jitter", target_fails),
            &target_fails,
            |b, &target_fails| {
                let attempts = Arc::new(AtomicU32::new(0));

                b.to_async(&rt).iter(|| {
                    let attempts = Arc::clone(&attempts);
                    attempts.store(0, Ordering::Relaxed);

                    retry_with_error(
                        Arc::clone(&attempts),
                        target_fails,
                        42,
                        config_with_full_jitter,
                    )
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("bounded_jitter", target_fails),
            &target_fails,
            |b, &target_fails| {
                let attempts = Arc::new(AtomicU32::new(0));

                b.to_async(&rt).iter(|| {
                    let attempts = Arc::clone(&attempts);
                    attempts.store(0, Ordering::Relaxed);

                    retry_with_error(
                        Arc::clone(&attempts),
                        target_fails,
                        42,
                        config_with_bounded_jitter,
                    )
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    benches_retry_happy_path,
    benches_retry_sad_path,
    bench_retry_sad_path_with_jitter
);
criterion_main!(benches);
