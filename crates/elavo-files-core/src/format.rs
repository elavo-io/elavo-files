//! Sizes and dates the way Finder writes them.

/// Decimal units, as Finder uses: `Zero bytes`, `1 byte`, `512 bytes`,
/// `84 KB`, `1.2 MB`, `3.45 GB`.
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    match bytes {
        0 => "Zero bytes".to_owned(),
        1 => "1 byte".to_owned(),
        b if b < 1_000 => format!("{b} bytes"),
        b => {
            let b = b as f64;
            if b < 999_500.0 {
                format!("{:.0} KB", (b / KB).max(1.0))
            } else if b < 999_950_000.0 {
                format!("{:.1} MB", b / (KB * KB))
            } else if b < 999_995_000_000.0 {
                format!("{:.2} GB", b / (KB * KB * KB))
            } else {
                format!("{:.2} TB", b / (KB * KB * KB * KB))
            }
        }
    }
}

/// `untitled folder`, or `untitled folder 2` and up when that is taken, as
/// Finder and Folio's desktop name a new folder.
pub fn new_folder_name(existing: &[String]) -> String {
    const BASE: &str = "untitled folder";
    if !existing.iter().any(|n| n == BASE) {
        return BASE.to_owned();
    }
    (2..)
        .map(|i| format!("{BASE} {i}"))
        .find(|name| !existing.iter().any(|n| n == name))
        .unwrap_or_else(|| BASE.to_owned())
}

/// What the Size column shows for a folder or anything without a size.
pub const NO_SIZE: &str = "--";

/// A calendar date and a wall-clock time, local to the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct When {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `4:01 PM`
pub fn time(when: &When) -> String {
    let (h, suffix) = match when.hour {
        0 => (12, "AM"),
        h @ 1..=11 => (h, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    format!("{h}:{:02} {suffix}", when.minute)
}

/// `Apr 15, 2026 at 4:01 PM`, or `Today at 4:01 PM` and
/// `Yesterday at 9:30 AM` the way Finder's list view shortens them.
/// `today` and `yesterday` are (year, month, day).
pub fn date(when: &When, today: (i32, u32, u32), yesterday: (i32, u32, u32)) -> String {
    let day = (when.year, when.month, when.day);
    if day == today {
        format!("Today at {}", time(when))
    } else if day == yesterday {
        format!("Yesterday at {}", time(when))
    } else {
        let month = MONTHS
            .get(when.month.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("?");
        format!("{month} {}, {} at {}", when.day, when.year, time(when))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(size(0), "Zero bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1_000), "1 KB");
        assert_eq!(size(84_211), "84 KB");
        assert_eq!(size(999_499), "999 KB");
        assert_eq!(size(999_999), "1.0 MB");
        assert_eq!(size(1_234_567), "1.2 MB");
        assert_eq!(size(3_450_000_000), "3.45 GB");
        assert_eq!(size(2_000_000_000_000), "2.00 TB");
    }

    #[test]
    fn new_folder_names_avoid_collisions() {
        assert_eq!(new_folder_name(&[]), "untitled folder");
        let taken = vec!["untitled folder".to_owned(), "untitled folder 2".to_owned()];
        assert_eq!(new_folder_name(&taken), "untitled folder 3");
    }

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> When {
        When {
            year,
            month,
            day,
            hour,
            minute,
        }
    }

    #[test]
    fn dates_read_like_finder() {
        let today = (2026, 11, 10);
        let yesterday = (2026, 11, 9);
        assert_eq!(
            date(&at(2026, 4, 15, 16, 1), today, yesterday),
            "Apr 15, 2026 at 4:01 PM"
        );
        assert_eq!(
            date(&at(2026, 11, 10, 0, 5), today, yesterday),
            "Today at 12:05 AM"
        );
        assert_eq!(
            date(&at(2026, 11, 9, 12, 30), today, yesterday),
            "Yesterday at 12:30 PM"
        );
        assert_eq!(
            date(&at(2025, 12, 31, 23, 59), today, yesterday),
            "Dec 31, 2025 at 11:59 PM"
        );
    }
}
