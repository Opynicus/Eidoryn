//! Bounded CUE parser for BINARY files, raw data tracks and audio tracks.
//! INDEX positions address sectors within their FILE, not within the whole disc.
use crate::{
    EmuError,
    disc::{DiscImage, Track, TrackInfo, TrackKind, invalid},
};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct CueTrack {
    file: usize,
    number: u8,
    kind: TrackKind,
    index0: Option<u32>,
    index1: Option<u32>,
    pregap: Option<u32>,
}

fn position(text: &str) -> Result<u32, EmuError> {
    let mut parts = text.split(':');
    let [Some(minutes), Some(seconds), Some(frames)] = std::array::from_fn(|_| parts.next()) else {
        return Err(invalid("CUE time must be MM:SS:FF"));
    };
    if parts.next().is_some() {
        return Err(invalid("CUE time must be MM:SS:FF"));
    }
    let parse = |part: &str| part.parse::<u32>().map_err(|_| invalid("Invalid CUE time"));
    let (minutes, seconds, frames) = (parse(minutes)?, parse(seconds)?, parse(frames)?);
    if minutes > 99 || seconds >= 60 || frames >= 75 {
        return Err(invalid("CUE time is out of range"));
    }
    Ok(minutes * 4500 + seconds * 75 + frames)
}

fn filename(text: &str) -> Result<PathBuf, EmuError> {
    let (name, kind) = if let Some(quoted) = text.strip_prefix('"') {
        quoted
            .split_once('"')
            .ok_or_else(|| invalid("Unclosed CUE filename quote"))?
    } else {
        text.split_once(char::is_whitespace)
            .ok_or_else(|| invalid("CUE FILE requires a type"))?
    };
    if !kind.trim().eq_ignore_ascii_case("BINARY") {
        return Err(invalid("Only BINARY CUE files are supported"));
    }
    let path = PathBuf::from(name.replace('\\', "/"));
    if name.is_empty()
        || name.contains(':')
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid(
            "CUE FILE must be a relative path below the CUE directory",
        ));
    }
    Ok(path)
}

pub(crate) fn open(path: &Path) -> Result<DiscImage, EmuError> {
    let file = File::open(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    if !file
        .metadata()
        .map_err(|e| invalid(e.to_string()))?
        .is_file()
    {
        return Err(invalid("CUE must be a regular file"));
    }
    let mut text = String::new();
    file.take(65_537)
        .read_to_string(&mut text)
        .map_err(|e| invalid(e.to_string()))?;
    if text.len() > 65_536 {
        return Err(invalid("CUE exceeds 64 KiB"));
    }
    CueSheet::parse(&text, path.parent().unwrap_or(Path::new(".")))?.open()
}

/// Parsed declarations contain no open files. Resolve sources only after all syntax is valid.
struct CueSheet {
    paths: Vec<PathBuf>,
    tracks: Vec<CueTrack>,
}

impl CueSheet {
    fn parse(text: &str, directory: &Path) -> Result<Self, EmuError> {
        let mut paths = Vec::new();
        let mut tracks: Vec<CueTrack> = Vec::new();
        let mut current_file = None;
        for (line_number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (directive, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let mut args = rest.split_whitespace();
            let fail = || {
                invalid(format!(
                    "Invalid CUE {} at line {}",
                    directive,
                    line_number + 1
                ))
            };
            match directive.to_ascii_uppercase().as_str() {
                "REM" | "TITLE" | "PERFORMER" | "SONGWRITER" | "CATALOG" | "ISRC" => {}
                "FILE" => {
                    paths.push(directory.join(filename(rest.trim())?));
                    current_file = Some(paths.len() - 1);
                }
                "TRACK" => {
                    let (Some(number), Some(kind), None) = (args.next(), args.next(), args.next())
                    else {
                        return Err(fail());
                    };
                    if tracks.len() >= 99 {
                        return Err(fail());
                    }
                    let number: u8 = number.parse().map_err(|_| fail())?;
                    if usize::from(number) != tracks.len() + 1 {
                        return Err(invalid("CUE tracks must be consecutive, starting at 01"));
                    }
                    let kind = match kind.to_ascii_uppercase().as_str() {
                        "MODE1/2352" => TrackKind::Mode1,
                        "MODE2/2352" => TrackKind::Mode2,
                        "AUDIO" => TrackKind::Audio,
                        _ => {
                            return Err(invalid(
                                "CUE supports MODE1/2352, MODE2/2352 and AUDIO tracks",
                            ));
                        }
                    };
                    tracks.push(CueTrack {
                        file: current_file.ok_or_else(fail)?,
                        number,
                        kind,
                        ..CueTrack::default()
                    });
                }
                "INDEX" | "PREGAP" => {
                    let track = tracks.last_mut().ok_or_else(fail)?;
                    if Some(track.file) != current_file {
                        return Err(fail());
                    }
                    if directive.eq_ignore_ascii_case("PREGAP") {
                        let (Some(time), None) = (args.next(), args.next()) else {
                            return Err(fail());
                        };
                        if track.pregap.is_some() {
                            return Err(fail());
                        }
                        track.pregap = Some(position(time)?);
                    } else {
                        let (Some(index), Some(time), None) =
                            (args.next(), args.next(), args.next())
                        else {
                            return Err(fail());
                        };
                        let slot = match index {
                            "00" if track.index1.is_none() => &mut track.index0,
                            "01" => &mut track.index1,
                            _ => {
                                return Err(invalid(
                                    "Only ordered CUE INDEX 00 and INDEX 01 are supported",
                                ));
                            }
                        };
                        if slot.is_some() {
                            return Err(fail());
                        }
                        *slot = Some(position(time)?);
                    }
                }
                _ => return Err(invalid(format!("Unsupported CUE directive {directive}"))),
            }
        }
        if tracks.is_empty() {
            return Err(invalid("CUE has no tracks"));
        }
        Ok(Self { paths, tracks })
    }

    fn open(self) -> Result<DiscImage, EmuError> {
        let Self { paths, tracks } = self;
        let mut sources = Vec::new();
        for (index, source) in paths.iter().enumerate() {
            if !tracks.iter().any(|t| t.file == index) {
                return Err(invalid("CUE FILE has no TRACK"));
            }
            let file =
                File::open(source).map_err(|e| invalid(format!("{}: {e}", source.display())))?;
            let metadata = file.metadata().map_err(|e| invalid(e.to_string()))?;
            if !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() % 2352 != 0
                || metadata.len() / 2352 > 359_849
            {
                return Err(invalid(format!(
                    "CUE BIN must contain complete 2352-byte sectors: {}",
                    source.display()
                )));
            }
            sources.push((Arc::new(Mutex::new(file)), (metadata.len() / 2352) as u32));
        }
        let mut result = Vec::new();
        let mut cursor = 0_u32;
        for (index, track) in tracks.iter().enumerate() {
            let start = track
                .index1
                .ok_or_else(|| invalid("CUE track is missing INDEX 01"))?;
            let beginning = track.index0.unwrap_or(start);
            if beginning > start || (track.pregap.is_some() && track.index0.is_some()) {
                return Err(invalid("Conflicting CUE pregap/index positions"));
            }
            let end = match tracks.get(index + 1).filter(|next| next.file == track.file) {
                Some(next) => next
                    .index0
                    .or(next.index1)
                    .ok_or_else(|| invalid("CUE track is missing INDEX 01"))?,
                None => sources[track.file].1,
            };
            if start >= end || end > sources[track.file].1 {
                return Err(invalid(
                    "CUE track extent is empty, overlapping or beyond its BIN file",
                ));
            }
            // The first track's lead-in is outside the nonnegative logical LBA space.
            let offset = if index == 0 { start } else { beginning };
            let gap = if index == 0 {
                0
            } else {
                track.pregap.unwrap_or(0)
            };
            if gap != 0 && track.kind != TrackKind::Audio {
                return Err(invalid(
                    "Synthetic PREGAP is currently supported for audio tracks only",
                ));
            }
            let length = gap + end - offset;
            if cursor + length > 359_849 {
                return Err(invalid("CUE exceeds supported CD address range"));
            }
            result.push(Track {
                info: TrackInfo {
                    number: track.number,
                    kind: track.kind,
                    index0_lba: cursor,
                    start_lba: cursor + gap + start - offset,
                    end_lba: cursor + length,
                },
                file: Arc::clone(&sources[track.file].0),
                file_offset: offset,
                gap,
                cooked: false,
            });
            cursor += length;
        }
        Ok(DiscImage {
            tracks: result,
            sectors: cursor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_positions_keep_component_bounds_and_error_priority() {
        for (text, expected) in [("00:00:00", 0), ("01:02:03", 4653), ("99:59:74", 449999)] {
            assert_eq!(position(text).unwrap(), expected);
        }
        for (text, expected) in [
            ("00:00", "CUE time must be MM:SS:FF"),
            ("bad:00:00:00", "CUE time must be MM:SS:FF"),
            ("00::00", "Invalid CUE time"),
            ("-1:00:00", "Invalid CUE time"),
            ("100:bad:00", "Invalid CUE time"),
            ("100:00:00", "CUE time is out of range"),
            ("00:60:00", "CUE time is out of range"),
            ("00:00:75", "CUE time is out of range"),
        ] {
            assert_eq!(position(text).unwrap_err().to_string(), expected);
        }
    }

    #[test]
    fn declarations_parse_without_opening_files() {
        let sheet = CueSheet::parse(
            "\u{feff}rem metadata ignored\nfile \"sub dir/disc.bin\" binary\ntrack 01 mode2/2352\nindex 01 00:02:00\ntrack 02 audio\npregap 00:01:00\nindex 01 00:03:00",
            Path::new("missing-cue-directory"),
        ).unwrap();
        assert_eq!(
            sheet.paths,
            [Path::new("missing-cue-directory").join("sub dir/disc.bin")]
        );
        assert_eq!(sheet.tracks.len(), 2);
        assert_eq!(sheet.tracks[0].kind, TrackKind::Mode2);
        assert_eq!(sheet.tracks[0].index1, Some(150));
        assert_eq!(sheet.tracks[1].kind, TrackKind::Audio);
        assert_eq!(sheet.tracks[1].pregap, Some(75));
        assert_eq!(sheet.tracks[1].index1, Some(225));
    }
}
