use crate::styled;

pub fn format_timestamp(
    timestamp: std::time::SystemTime,
    test_start: bombadil_schema::Time,
) -> String {
    let time = bombadil_schema::Time::from_system_time(timestamp);
    let elapsed = std::time::Duration::from_micros(
        time.as_micros().saturating_sub(test_start.as_micros()),
    );
    styled::maybe_dimmed(bombadil_schema::duration::format_duration(
        elapsed,
        bombadil_schema::duration::FormatDurationOptions {
            include_millis: true,
        },
    ))
}
