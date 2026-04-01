use std::time::Duration;

pub fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

pub fn format_bound(duration: Duration) -> String {
    let milliseconds = duration.as_millis();

    if milliseconds == 0 {
        return "0 milliseconds".to_string();
    }

    if milliseconds % 60_000 == 0 {
        let minutes = milliseconds / 60_000;
        if minutes == 1 {
            "1 minute".to_string()
        } else {
            format!("{} minutes", minutes)
        }
    } else if milliseconds % 1_000 == 0 {
        let seconds = milliseconds / 1_000;
        if seconds == 1 {
            "1 second".to_string()
        } else {
            format!("{} seconds", seconds)
        }
    } else if milliseconds == 1 {
        "1 millisecond".to_string()
    } else {
        format!("{} milliseconds", milliseconds)
    }
}
