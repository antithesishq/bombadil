use std::time::Duration;

pub struct FormatDurationOptions {
    pub include_millis: bool,
}

impl Default for FormatDurationOptions {
    fn default() -> Self {
        Self {
            include_millis: true,
        }
    }
}

pub fn format_duration(
    duration: Duration,
    options: FormatDurationOptions,
) -> String {
    let total_secs = duration.as_secs();
    let millis_part = if options.include_millis {
        format!(".{:03}", duration.subsec_millis())
    } else {
        "".into()
    };
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}{}", hours, minutes, seconds, millis_part)
    } else {
        format!("{:02}:{:02}{}", minutes, seconds, millis_part)
    }
}
