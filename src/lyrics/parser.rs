//! Small, line-synced LRC parser. No UI or networking dependencies.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub timestamp: f64,
    pub text: String,
}

fn timestamp(value: &str) -> Option<f64> {
    let (minutes, seconds) = value.split_once(':')?;
    if minutes.is_empty() || !minutes.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let seconds: f64 = seconds.parse().ok()?;
    let minutes: f64 = minutes.parse().ok()?;
    let result = minutes * 60.0 + seconds;
    (seconds.is_finite() && (0.0..60.0).contains(&seconds) && result.is_finite()).then_some(result)
}

pub fn parse(lrc: &str) -> Vec<LyricLine> {
    // Offset applies to the entire document, even if declared after the lyrics.
    let offset = lrc
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("[offset:")?
                .strip_suffix(']')?
                .trim()
                .parse::<f64>()
                .ok()
        })
        .rfind(|n| n.is_finite())
        .unwrap_or(0.0)
        / 1000.0;
    let mut lines = Vec::new();
    for line in lrc.lines() {
        let mut rest = line.trim().trim_start_matches('\u{feff}');
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some((value, tail)) = tag.split_once(']') else {
                break;
            };
            let Some(time) = timestamp(value) else { break };
            times.push((time + offset).max(0.0));
            rest = tail;
        }
        for time in times {
            lines.push(LyricLine {
                timestamp: time,
                text: rest.trim().to_owned(),
            });
        }
    }
    lines.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    // Simultaneous translations/voices belong on the same displayed line.
    let mut merged: Vec<LyricLine> = Vec::new();
    for line in lines {
        if let Some(last) = merged
            .last_mut()
            .filter(|last| last.timestamp == line.timestamp)
        {
            if last.text != line.text && !line.text.is_empty() {
                if !last.text.is_empty() {
                    last.text.push('\n');
                }
                last.text.push_str(&line.text);
            }
        } else {
            merged.push(line);
        }
    }
    merged
}

pub fn active_line(lines: &[LyricLine], elapsed: f64) -> Option<usize> {
    if !elapsed.is_finite() || elapsed < 0.0 {
        return None;
    }
    lines
        .partition_point(|line| line.timestamp <= elapsed)
        .checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_offsets_repeats_and_metadata() {
        let lines = parse(
            "[ar:Artist]\n[00:12.345][01:02.5] Hello\n[00:03]First\n[offset:-500]\n[bad]ignore\n[00:99]bad",
        );
        assert_eq!(
            lines.iter().map(|l| l.timestamp).collect::<Vec<_>>(),
            vec![2.5, 11.845, 62.0]
        );
        assert_eq!(lines[1].text, "Hello");
    }
    #[test]
    fn seek_forward_backward_and_pause() {
        let lines = parse("[00:05]One\n[00:10]Two\n[00:20]");
        for (elapsed, expected) in [
            (0.0, None),
            (5.0, Some(0)),
            (18.0, Some(1)),
            (6.0, Some(0)),
            (6.0, Some(0)),
            (40.0, Some(2)),
        ] {
            assert_eq!(active_line(&lines, elapsed), expected);
        }
        assert_eq!(active_line(&lines, f64::NAN), None);
        assert_eq!(active_line(&[], 12.0), None);
    }
    #[test]
    fn simultaneous_and_invalid_lines() {
        assert_eq!(parse("[00:01]A\n[00:01]B")[0].text, "A\nB");
        assert!(parse("[ti:Title]\n[00:NaN]bad\n[00:-1]bad\nplain").is_empty());
    }
}
