use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, time::Duration};
use url::Url;

#[derive(Clone, Debug)]
pub struct Part {
    pub url: Url,
    range: Option<(u64, u64)>,
    pub initialization: Option<Box<Part>>,
}

impl Part {
    pub fn headers(&self) -> String {
        match self.range {
            Some((start, end)) => format!("Range: bytes={start}-{end}\r\n"),
            None => String::new(),
        }
    }
}

pub struct Playlist {
    pub variant: Option<Url>,
    pub sequence: u64,
    pub segments: Vec<Part>,
    pub wait: Duration,
    pub ended: bool,
}

pub fn parse(base: &Url, text: &str) -> Result<Playlist> {
    if !text.trim_start_matches('\u{feff}').starts_with("#EXTM3U") {
        bail!("Invalid M3U8 playlist");
    }
    let mut playlist = Playlist {
        variant: None,
        sequence: 0,
        segments: Vec::new(),
        wait: Duration::from_secs(3),
        ended: false,
    };
    let mut variants = Vec::new();
    let mut renditions = BTreeMap::new();
    let mut stream_info = None;
    let mut init = None;
    let mut byte_range = None;
    let mut previous_end = 0;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            stream_info = Some((
                attribute(attributes, "BANDWIDTH")
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(u64::MAX),
                attribute(attributes, "AUDIO").map(str::to_owned),
            ));
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-MEDIA:") {
            if attribute(attributes, "TYPE") == Some("AUDIO") {
                if let (Some(group), Some(uri)) = (
                    attribute(attributes, "GROUP-ID"),
                    attribute(attributes, "URI"),
                ) {
                    let url = base.join(uri)?;
                    let default = attribute(attributes, "DEFAULT") == Some("YES");
                    if default || !renditions.contains_key(group) {
                        renditions.insert(group.to_owned(), url);
                    }
                }
            }
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-KEY:") {
            if attribute(attributes, "METHOD") != Some("NONE") {
                bail!("Encrypted HLS stream");
            }
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-MAP:") {
            let url =
                base.join(attribute(attributes, "URI").context("Missing HLS initialization URL")?)?;
            let range = attribute(attributes, "BYTERANGE")
                .map(|range| parse_range(range, 0))
                .transpose()?;
            init = Some(Box::new(Part {
                url,
                range,
                initialization: None,
            }));
        } else if let Some(raw) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            playlist.sequence = raw.parse()?;
        } else if let Some(raw) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            let seconds: u64 = raw.parse()?;
            playlist.wait = Duration::from_millis((seconds.saturating_mul(500)).clamp(500, 4_000));
        } else if let Some(raw) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            byte_range = Some(parse_range(raw, previous_end)?);
        } else if line == "#EXT-X-ENDLIST" {
            playlist.ended = true;
        } else if !line.starts_with('#') {
            let url = base.join(line)?;
            if let Some((bandwidth, audio)) = stream_info.take() {
                variants.push((bandwidth, audio, url));
            } else {
                previous_end = byte_range.map_or(0, |(_, end)| end.saturating_add(1));
                playlist.segments.push(Part {
                    url,
                    range: byte_range.take(),
                    initialization: init.clone(),
                });
            }
        }
    }
    if let Some((_, group, url)) = variants.into_iter().min_by_key(|variant| variant.0) {
        playlist.variant = Some(
            group
                .and_then(|group| renditions.get(&group).cloned())
                .unwrap_or(url),
        );
    }
    Ok(playlist)
}

fn parse_range(text: &str, default_start: u64) -> Result<(u64, u64)> {
    let (length, start) = match text.split_once('@') {
        Some((length, start)) => (length.parse::<u64>()?, start.parse()?),
        None => (text.parse::<u64>()?, default_start),
    };
    if length == 0 {
        bail!("Empty HLS byte range");
    }
    Ok((
        start,
        start
            .checked_add(length - 1)
            .context("Invalid HLS byte range")?,
    ))
}

fn attribute<'a>(mut attributes: &'a str, key: &str) -> Option<&'a str> {
    while let Some((name, tail)) = attributes.split_once('=') {
        let tail = tail.trim_start();
        let (value, remaining) = if let Some(quoted) = tail.strip_prefix('"') {
            let (value, remaining) = quoted.split_once('"')?;
            (value, remaining.trim_start_matches(','))
        } else {
            tail.split_once(',').unwrap_or((tail, ""))
        };
        if name.trim() == key {
            return Some(value);
        }
        attributes = remaining;
    }
    None
}

pub fn transport_audio(data: &[u8]) -> Result<(Vec<u8>, &'static str)> {
    let sync = (0..data.len().min(188))
        .find(|i| data.get(*i) == Some(&0x47) && data.get(i + 188) == Some(&0x47))
        .context("MPEG transport stream is out of sync")?;
    let mut program = None;
    let mut audio = None;
    let mut output = Vec::new();
    for packet in data[sync..].chunks_exact(188) {
        if packet[0] != 0x47 || packet[1] & 0x80 != 0 {
            continue;
        }
        let pid = u16::from_be_bytes([packet[1] & 0x1f, packet[2]]);
        let begin = packet[1] & 0x40 != 0;
        let offset = match (packet[3] >> 4) & 3 {
            1 => 4,
            3 => 5 + packet[4] as usize,
            _ => continue,
        };
        let Some(payload) = packet.get(offset..) else {
            continue;
        };
        if begin && (pid == 0 || Some(pid) == program) {
            let Some(section) = payload
                .first()
                .and_then(|skip| payload.get(1 + *skip as usize..))
            else {
                continue;
            };
            let Some(length) = section.get(1..3) else {
                continue;
            };
            let end = 3 + (u16::from_be_bytes([length[0] & 15, length[1]]) as usize);
            let Some(table) = section.get(..end.saturating_sub(4)) else {
                continue;
            };
            if pid == 0 && table.first() == Some(&0) {
                program = table
                    .get(8..)
                    .and_then(|rows| rows.chunks_exact(4).find(|row| row[0] != 0 || row[1] != 0))
                    .map(|row| u16::from_be_bytes([row[2] & 31, row[3]]));
            } else if table.first() == Some(&2) && table.len() >= 12 {
                let offset = 12 + u16::from_be_bytes([table[10] & 15, table[11]]) as usize;
                let mut rows = table.get(offset..).unwrap_or_default();
                while rows.len() >= 5 {
                    let format = match rows[0] {
                        0x0f => Some("aac"),
                        3 | 4 => Some("mp3"),
                        _ => None,
                    };
                    if let Some(format) = format {
                        audio = Some((u16::from_be_bytes([rows[1] & 31, rows[2]]), format));
                        break;
                    }
                    let next = 5 + u16::from_be_bytes([rows[3] & 15, rows[4]]) as usize;
                    rows = rows.get(next..).unwrap_or_default();
                }
            }
        } else if audio.is_some_and(|(track, _)| pid == track) {
            let payload = if begin {
                if !payload.starts_with(&[0, 0, 1]) || payload.len() < 9 {
                    continue;
                }
                payload.get(9 + payload[8] as usize..).unwrap_or_default()
            } else {
                payload
            };
            output.extend_from_slice(payload);
        }
    }
    if output.is_empty() {
        bail!("No supported audio track in MPEG transport stream");
    }
    Ok((output, audio.context("Missing audio track")?.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_prefers_default_audio_and_preserves_byte_ranges() {
        let base = Url::parse("https://test.invalid/radio/master.m3u8").unwrap();
        let master = parse(&base, "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"radio\",DEFAULT=YES,URI=\"audio/live.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=80000,AUDIO=\"radio\"\nvideo.m3u8").unwrap();
        assert_eq!(
            master.variant.unwrap().as_str(),
            "https://test.invalid/radio/audio/live.m3u8"
        );
        let media = parse(&base, "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:30\n#EXT-X-MAP:URI=\"data.m4a\",BYTERANGE=\"25@0\"\n#EXT-X-BYTERANGE:30@25\ndata.m4a\n#EXT-X-BYTERANGE:10\ndata.m4a\n#EXT-X-ENDLIST").unwrap();
        assert_eq!(media.sequence, 30);
        assert!(media.ended);
        assert_eq!(media.segments[1].headers(), "Range: bytes=55-64\r\n");
        assert_eq!(
            media.segments[1].initialization.as_ref().unwrap().headers(),
            "Range: bytes=0-24\r\n"
        );
    }
}
