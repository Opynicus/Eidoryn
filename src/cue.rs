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
    let parts: Vec<_> = text.split(':').collect();
    if parts.len() != 3 {
        return Err(invalid("CUE time must be MM:SS:FF"));
    }
    let n = parts
        .iter()
        .map(|s| s.parse::<u32>().map_err(|_| invalid("Invalid CUE time")))
        .collect::<Result<Vec<_>, _>>()?;
    if n[0] > 99 || n[1] >= 60 || n[2] >= 75 {
        return Err(invalid("CUE time is out of range"));
    }
    Ok(n[0] * 4500 + n[1] * 75 + n[2])
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

#[allow(clippy::too_many_lines)] // Parse declarations, validate all extents, then publish the image.
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
    let mut paths = Vec::new();
    let mut tracks: Vec<CueTrack> = Vec::new();
    let mut current_file = None;
    for (line_number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (directive, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let args: Vec<_> = rest.split_whitespace().collect();
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
                paths.push(
                    path.parent()
                        .unwrap_or(Path::new("."))
                        .join(filename(rest.trim())?),
                );
                current_file = Some(paths.len() - 1);
            }
            "TRACK" => {
                if args.len() != 2 || tracks.len() >= 99 {
                    return Err(fail());
                }
                let number: u8 = args[0].parse().map_err(|_| fail())?;
                if usize::from(number) != tracks.len() + 1 {
                    return Err(invalid("CUE tracks must be consecutive, starting at 01"));
                }
                let kind = match args[1].to_ascii_uppercase().as_str() {
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
                    if args.len() != 1 || track.pregap.is_some() {
                        return Err(fail());
                    }
                    track.pregap = Some(position(args[0])?);
                } else {
                    if args.len() != 2 {
                        return Err(fail());
                    }
                    let slot = match args[0] {
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
                    *slot = Some(position(args[1])?);
                }
            }
            _ => return Err(invalid(format!("Unsupported CUE directive {directive}"))),
        }
    }
    if tracks.is_empty() {
        return Err(invalid("CUE has no tracks"));
    }
    let mut sources = Vec::new();
    for (index, source) in paths.iter().enumerate() {
        if !tracks.iter().any(|t| t.file == index) {
            return Err(invalid("CUE FILE has no TRACK"));
        }
        let file = File::open(source).map_err(|e| invalid(format!("{}: {e}", source.display())))?;
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
