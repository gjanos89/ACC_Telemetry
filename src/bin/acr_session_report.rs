use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use acr_recorder::export::rkyv_reader::{read_graphics_rkyv, read_rkyv};
use serde_json::json;

fn graphics_sidecar(physics: &Path) -> PathBuf {
    let stem = physics.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    physics.parent().unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.graphics.rkyv"))
}

fn json_sidecar(physics: &Path) -> PathBuf {
    let stem = physics.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    physics.parent().unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.json"))
}

// ACC format: M:SS:mmm, e.g. 1:51:592 = 111.592 seconds.
fn parse_acc_time(s: &str) -> Option<f64> {
    let p: Vec<&str> = s.trim().split(':').collect();
    if p.len() != 3 { return None; }
    let min: f64 = p[0].parse().ok()?;
    let sec: f64 = p[1].parse().ok()?;
    let ms: f64 = p[2].parse().ok()?;
    Some(min * 60.0 + sec + ms / 1000.0)
}

fn fmt_sec(sec: f64) -> String {
    if !sec.is_finite() || sec <= 0.0 { return "â”śĂłĂ”Ă©ÄŚĂ”Ă‡Ĺ".into(); }
    let min = (sec / 60.0).floor();
    let s = sec - min * 60.0;
    format!("{:02.0}:{:06.3}", min, s)
}

fn clean_label(s: &str) -> String {
    let mut out = s.replace('_', " ").replace('-', " ");
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out
}

fn find_string(v: &serde_json::Value, wanted: &[&str]) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for key in wanted {
                if let Some(x) = map.get(*key) {
                    if let Some(s) = x.as_str() {
                        if !s.trim().is_empty() { return Some(s.to_string()); }
                    }
                }
            }
            for child in map.values() {
                if let Some(s) = find_string(child, wanted) { return Some(s); }
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                if let Some(s) = find_string(child, wanted) { return Some(s); }
            }
        }
        _ => {}
    }
    None
}

fn read_metadata(physics: &Path) -> (String, String) {
    let p = json_sidecar(physics);
    let raw = match fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("metadata: cannot read {}: {}", p.display(), e);
            return ("Unknown track".into(), "Unknown car".into());
        }
    };
    let root: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("metadata: invalid JSON {}: {}", p.display(), e);
            return ("Unknown track".into(), "Unknown car".into());
        }
    };

    let track = find_string(&root, &["track", "track_name", "trackName", "track_id", "trackId"])
        .map(|s| clean_label(&s))
        .unwrap_or_else(|| "Unknown track".into());
    let car = find_string(&root, &["car_model", "carModel", "car", "vehicle_model", "vehicleModel"])
        .map(|s| clean_label(&s))
        .unwrap_or_else(|| "Unknown car".into());

    (track, car)
}

#[derive(Clone)]
struct Lap {
    number: i32,
    gfx_start: usize,
    gfx_end: usize,
    time_str: String,
    time_sec: f64,
    valid: bool,
}


fn monza_brake_corner(index: usize, track_name: &str) -> Option<i32> {
    if track_name.to_lowercase().contains("monza") {
        // Monza's five principal braking zones in lap order. The chicane
        // components are kept under the official turn numbering: 1, 4, 6, 8, 11.
        [1, 4, 6, 8, 11].get(index).copied()
    } else {
        None
    }
}

fn nearest_brake_corner(progress: f64, brakes: &[serde_json::Value]) -> Option<i32> {
    let mut best: Option<(f64, i32)> = None;
    for b in brakes {
        let bp = b.get("progress").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let corner = b.get("corner").and_then(|v| v.as_i64()).map(|v| v as i32);
        if let Some(c) = corner {
            let d = (bp - progress).abs();
            if best.map(|x| d < x.0).unwrap_or(true) { best = Some((d, c)); }
        }
    }
    best.and_then(|(d,c)| if d <= 12.0 { Some(c) } else { None })
}


fn nearest_index_by_time(
    physics: &[acr_recorder::record::PhysicsRecord],
    t: f64,
) -> usize {
    if physics.is_empty() { return 0; }
    let mut lo = 0usize;
    let mut hi = physics.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if physics[mid].capture_time_sec < t { lo = mid + 1; } else { hi = mid; }
    }
    if lo == 0 { return 0; }
    if lo >= physics.len() { return physics.len() - 1; }
    let a = physics[lo - 1].capture_time_sec;
    let b = physics[lo].capture_time_sec;
    if (t - a).abs() <= (b - t).abs() { lo - 1 } else { lo }
}

fn nearest_gfx_index_by_time(
    gfx: &[acr_recorder::record::GraphicsRecord],
    gfx_hz: u32,
    t: f64,
    start: usize,
    end: usize,
) -> usize {
    if gfx.is_empty() { return 0; }
    let last = gfx.len() - 1;
    let lo_bound = start.min(last);
    let hi_bound = end.min(last);
    if lo_bound >= hi_bound { return lo_bound; }

    let pos = (t * gfx_hz as f64).round();
    if !pos.is_finite() {
        return lo_bound;
    }
    let target = (pos as isize).clamp(lo_bound as isize, hi_bound as isize) as usize;

    // Graphics samples are uniformly sampled. Clamp the result to the
    // requested lap range; no packet-id matching is used between streams.
    target
}

fn wall_secs_from_notes(notes_path: &Path) -> Option<f64> {
    let raw = fs::read_to_string(notes_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let a = chrono::DateTime::parse_from_rfc3339(v["recording_start_utc"].as_str()?).ok()?;
    let b = chrono::DateTime::parse_from_rfc3339(v["recording_end_utc"].as_str()?).ok()?;
    Some((b - a).num_milliseconds() as f64 / 1000.0)
}

fn effective_physics_hz(physics: &[acr_recorder::record::PhysicsRecord], physics_path: &Path, header_hz: u32) -> f64 {
    if physics.is_empty() { return header_hz.max(1) as f64; }
    let notes = physics_path.parent().unwrap_or_else(|| Path::new(".")).join(
        format!("{}.notes.json", physics_path.file_stem().and_then(|s| s.to_str()).unwrap_or(""))
    );
    if let Some(wall) = wall_secs_from_notes(&notes) {
        if wall > 1.0 { return physics.len() as f64 / wall; }
    }
    header_hz.max(1) as f64
}

fn physics_distance_cum(physics: &[acr_recorder::record::PhysicsRecord]) -> Vec<f64> {
    let mut out = Vec::with_capacity(physics.len());
    if physics.is_empty() { return out; }
    out.push(0.0);
    for i in 1..physics.len() {
        let dt = (physics[i].capture_time_sec - physics[i - 1].capture_time_sec).max(0.0);
        let v0 = physics[i - 1].speed_kmh.max(0.0) as f64 / 3.6;
        let v1 = physics[i].speed_kmh.max(0.0) as f64 / 3.6;
        let prev = *out.last().unwrap();
        out.push(prev + 0.5 * (v0 + v1) * dt);
    }
    out
}

fn nearest_distance_index(dist: &[f64], target: f64) -> usize {
    if dist.is_empty() { return 0; }
    let mut lo = 0usize;
    let mut hi = dist.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if dist[mid] < target { lo = mid + 1; } else { hi = mid; }
    }
    if lo == 0 { return 0; }
    if lo >= dist.len() { return dist.len() - 1; }
    if (dist[lo] - target).abs() < (dist[lo - 1] - target).abs() { lo } else { lo - 1 }
}

#[derive(Clone, Copy)]
struct PhysicsLapRange {
    start: usize,
    end: usize,
}

fn find_first_physics_lap_start(
    physics: &[acr_recorder::record::PhysicsRecord],
    p_dist: &[f64],
    gfx: &[acr_recorder::record::GraphicsRecord],
    lap: &Lap,
) -> usize {
    if physics.is_empty() || p_dist.is_empty() { return 0; }
    let gfx_start_distance = gfx[lap.gfx_start].distance_traveled as f64;
    let gfx_end_distance = gfx[lap.gfx_end].distance_traveled as f64;
    let gfx_lap_distance = (gfx_end_distance - gfx_start_distance).abs();
    let drive_start = find_physics_drive_start(physics)
        .map(|t| nearest_index_by_time(physics, t))
        .unwrap_or(0);

    // The first completed lap is located by two independent quantities that
    // must agree: elapsed lap time and travelled lap distance.  This avoids
    // assuming a fixed stream offset or a nominal 333/60 Hz ratio.
    let max_start = physics.len().saturating_sub(2);
    let mut best = (f64::INFINITY, drive_start);
    let step = 4usize;
    for i in (drive_start..max_start).step_by(step) {
        let target_time = physics[i].capture_time_sec + lap.time_sec;
        let j = nearest_index_by_time(physics, target_time);
        if j <= i { continue; }
        let dt = physics[j].capture_time_sec - physics[i].capture_time_sec;
        let dd = p_dist[j] - p_dist[i];
        let time_err = (dt - lap.time_sec).abs();
        let dist_err = (dd - gfx_lap_distance).abs();
        let score = dist_err + time_err * 20.0;
        if score < best.0 {
            best = (score, i);
        }
    }

    eprintln!(
        "Telemetry lap alignment: first lap start physics={} distance={:.2} m, graphics lap distance={:.2} m, score={:.3}",
        best.1, p_dist[best.1], gfx_lap_distance, best.0
    );
    best.1
}

fn build_physics_lap_ranges(
    physics: &[acr_recorder::record::PhysicsRecord],
    p_dist: &[f64],
    gfx: &[acr_recorder::record::GraphicsRecord],
    laps: &[Lap],
) -> Vec<PhysicsLapRange> {
    if laps.is_empty() || physics.is_empty() || p_dist.is_empty() {
        return Vec::new();
    }

    let first_start = find_first_physics_lap_start(physics, p_dist, gfx, &laps[0]);
    let mut ranges: Vec<PhysicsLapRange> = Vec::with_capacity(laps.len());
    let mut start_distance = p_dist[first_start];

    for (i, lap) in laps.iter().enumerate() {
        if i > 0 {
            start_distance = p_dist[ranges[i - 1].start]
                + (gfx[laps[i - 1].gfx_end].distance_traveled as f64
                    - gfx[laps[i - 1].gfx_start].distance_traveled as f64).abs();
        }
        let lap_distance = (gfx[lap.gfx_end].distance_traveled as f64
            - gfx[lap.gfx_start].distance_traveled as f64).abs();
        let end_distance = start_distance + lap_distance;
        let start = nearest_distance_index(p_dist, start_distance);
        let end = nearest_distance_index(p_dist, end_distance).max(start);
        ranges.push(PhysicsLapRange { start, end });
    }
    ranges
}

fn find_physics_drive_start(
    physics: &[acr_recorder::record::PhysicsRecord],
) -> Option<f64> {
    if physics.is_empty() { return None; }

    // Find the first sustained movement rather than a single noisy speed sample.
    // This is used only to estimate the relative start of the two telemetry
    // streams; the actual lap boundaries come from Graphics.completed_lap.
    let required = 100usize; // ~0.30 s at 333 Hz
    for i in 0..physics.len() {
        if physics[i].speed_kmh < 20.0 { continue; }
        let end = (i + required).min(physics.len());
        if end - i < required { break; }
        let moving = physics[i..end].iter()
            .filter(|r| r.speed_kmh >= 15.0)
            .count();
        if moving >= required * 9 / 10 {
            return Some(physics[i].capture_time_sec);
        }
    }
    None
}

fn estimate_stream_offset(
    physics: &[acr_recorder::record::PhysicsRecord],
    gfx: &[acr_recorder::record::GraphicsRecord],
    gfx_hz: u32,
    first_lap_end: usize,
) -> f64 {
    let gfx_start = find_lap_start_gfx(gfx, 0, first_lap_end);
    let gfx_start_time = gfx_start as f64 / gfx_hz.max(1) as f64;
    let physics_start = find_physics_drive_start(physics).unwrap_or(0.0);

    // stream_offset is defined as:
    //     graphics_time = physics_time + stream_offset
    // Therefore a physics sample is mapped to graphics at
    //     graphics_time = physics.capture_time_sec + stream_offset.
    let offset = gfx_start_time - physics_start;

    eprintln!(
        "Telemetry time alignment: gfx lap-1 start={:.3}s, physics drive start={:.3}s, offset={:+.3}s",
        gfx_start_time, physics_start, offset
    );

    if offset.is_finite() && offset.abs() < 600.0 { offset } else { 0.0 }
}

fn find_lap_start_gfx(
    gfx: &[acr_recorder::record::GraphicsRecord],
    previous_end: usize,
    current_end: usize,
) -> usize {
    if gfx.is_empty() || current_end <= previous_end {
        return previous_end.min(gfx.len().saturating_sub(1));
    }

    // completed_lap is the finish boundary, but it is not a reliable lap-start
    // boundary across pit stops. ACC can keep completed_lap unchanged while the
    // car leaves the pits and crosses start/finish again. Use the LAST genuine
    // start/finish crossing before the completed-lap boundary instead.
    //
    // For ordinary consecutive laps there is no extra crossing between the two
    // completed-lap boundaries, so the previous boundary itself is the start.
    let hi = current_end.saturating_sub(1).min(gfx.len().saturating_sub(1));
    let lo = previous_end.saturating_add(1).min(hi);

    if lo <= hi {
        for i in (lo..=hi).rev() {
            if gfx[i].is_in_pit_lane {
                continue;
            }
            let prev = &gfx[i - 1];
            let p = prev.normalized_car_position as f64;
            let n = gfx[i].normalized_car_position as f64;
            if p >= 0.90 && n <= 0.10 {
                return i;
            }
        }
    }

    previous_end.min(gfx.len().saturating_sub(1))
}

fn sector_time_sec(g: &acr_recorder::record::GraphicsRecord) -> Option<f64> {
    if g.last_sector_time > 0 {
        return Some(g.last_sector_time as f64 / 1000.0);
    }
    parse_acc_time(&g.last_sector_time_str)
}

fn extract_lap_sectors(
    gfx: &[acr_recorder::record::GraphicsRecord],
    start: usize,
    end: usize,
    lap_time_sec: f64,
) -> Vec<Option<f64>> {
    let mut sectors = vec![None, None, None];

    if gfx.is_empty() || start >= gfx.len() || end <= start {
        return sectors;
    }

    // ACC's last_sector_time is cumulative at the sector boundary:
    //
    // S1 = cumulative S1
    // S2 = cumulative S2 - cumulative S1
    // S3 = completed lap time - cumulative S2
    let mut cumulative_s1 = None;
    let mut cumulative_s2 = None;
    let mut previous = gfx[start].current_sector_index;

    for i in (start + 1)..=end.min(gfx.len() - 1) {
        let current = gfx[i].current_sector_index;

        if current != previous {
            if let Some(t) = sector_time_sec(&gfx[i]) {
                match previous {
                    0 => cumulative_s1 = Some(t),
                    1 => cumulative_s2 = Some(t),
                    _ => {}
                }
            }
            previous = current;
        }
    }

    if let Some(s1) = cumulative_s1 {
        sectors[0] = Some(s1);
    }

    if let (Some(s1), Some(s2cum)) = (cumulative_s1, cumulative_s2) {
        sectors[1] = Some((s2cum - s1).max(0.0));

        // IMPORTANT:
        // Use the already validated completed lap time.
        // Do NOT use Graphics.last_time_str here.
        if lap_time_sec.is_finite() && lap_time_sec > 0.0 {
            sectors[2] = Some((lap_time_sec - s2cum).max(0.0));
        }
    }

    sectors
}

fn event_points(
    selected: &[&acr_recorder::record::PhysicsRecord],
    physics: &[acr_recorder::record::PhysicsRecord],
    p_dist: &[f64],
    p_start: usize,
    lap_time_sec: f64,
    lap_distance: f64,
    track_name: &str,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut brakes = Vec::new();
    let mut throttles = Vec::new();
    let mut prev_brake = 0.0f32;
    let mut prev_gas = 0.0f32;
    let mut last_brake_dist = -10_000.0f64;
    let mut last_throttle_dist = -10_000.0f64;
    let lap_start_time = physics.get(p_start).map(|r| r.capture_time_sec).unwrap_or(0.0);

    for r in selected {
        let physics_index = nearest_index_by_time(physics, r.capture_time_sec);
        let dist = if physics_index < p_dist.len() && p_start < p_dist.len() {
            (p_dist[physics_index] - p_dist[p_start]).max(0.0)
        } else { 0.0 };
        let progress = if lap_distance > 0.0 {
            (dist / lap_distance * 100.0).clamp(0.0, 100.0)
        } else { 0.0 };
        let event_time = (r.capture_time_sec - lap_start_time)
            .clamp(0.0, lap_time_sec.max(0.0));

        if r.brake >= 0.10 && prev_brake < 0.05 && dist - last_brake_dist >= 80.0 {
            let corner = monza_brake_corner(brakes.len(), track_name);
            if !track_name.to_lowercase().contains("monza") || corner.is_some() {
                brakes.push(json!({
                    "corner": corner,
                    "progress": progress,
                    "distance": dist,
                    "time": event_time,
                    "speed": r.speed_kmh
                }));
                last_brake_dist = dist;
            }
        }

        if !brakes.is_empty() && r.gas >= 0.50 && prev_gas < 0.20 && dist - last_throttle_dist >= 80.0 {
            let corner = nearest_brake_corner(progress, &brakes);
            throttles.push(json!({
                "corner": corner,
                "progress": progress,
                "distance": dist,
                "time": event_time,
                "speed": r.speed_kmh
            }));
            last_throttle_dist = dist;
        }

        prev_brake = r.brake;
        prev_gas = r.gas;
    }

    (brakes, throttles)
}

fn best_ever_db_path(physics: &Path) -> PathBuf {
    physics.parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| Path::new("."))
        .join("best_ever.json")
}

fn load_best_ever(path: &Path, track_name: &str, car_name: &str) -> Option<serde_json::Value> {
    let raw = fs::read_to_string(path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let key = format!("{}|{}", track_name.to_lowercase(), car_name.to_lowercase());
    root.get("records")
        .and_then(|v| v.as_object())
        .and_then(|m| m.get(&key))
        .cloned()
}

fn save_best_ever(
    path: &Path,
    track_name: &str,
    car_name: &str,
    candidate: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut root = if path.exists() {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or_else(|| json!({"records": {}}))
    } else {
        json!({"records": {}})
    };

    let key = format!("{}|{}", track_name.to_lowercase(), car_name.to_lowercase());
    if root.get("records").and_then(|v| v.as_object()).is_none() {
        root["records"] = json!({});
    }
    root["records"][&key] = candidate.clone();

    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let physics = env::args().nth(1).map(PathBuf::from)
        .ok_or("Hasznâ”€Ă©â•¦Ă§lat: acr_session_report.exe <physics.rkyv> [output.html]")?;
    let out = env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        physics.parent().unwrap_or_else(|| Path::new("."))
            .join(format!("{}_report.html",
                physics.file_stem().and_then(|s| s.to_str()).unwrap_or("session")))
    });

    let gfx_path = graphics_sidecar(&physics);
    if !gfx_path.exists() {
        return Err(format!("Hiâ”€Ă©â•¦Ă§nyzik a graphics sidecar: {}", gfx_path.display()).into());
    }

    let (track_name, car_name) = read_metadata(&physics);
    eprintln!("Track: {}", track_name);
    eprintln!("Car: {}", car_name);

    eprintln!("Reading physics...");
    let (physics_hz, physics_records) = read_rkyv(&physics)?;
    eprintln!("Read {} physics samples at {} Hz", physics_records.len(), physics_hz);

    eprintln!("Reading graphics...");
    let (gfx_hz, gfx) = read_graphics_rkyv(&gfx_path)?;
    eprintln!("Read {} graphics samples at {} Hz", gfx.len(), gfx_hz);

    if gfx.is_empty() || physics_records.is_empty() {
        return Err("Nincs telemetry adat.".into());
    }

    // completed_lap == N is the authoritative finish boundary for lap N.
    let mut boundaries: Vec<(i32, usize)> = Vec::new();
    let mut last_seen_lap = 0;
    for (i, r) in gfx.iter().enumerate() {
        if r.completed_lap > last_seen_lap && r.completed_lap > 0 {
            boundaries.push((r.completed_lap, i));
            last_seen_lap = r.completed_lap;
        }
    }

    if boundaries.is_empty() {
        return Err("Nem talâ”śĂ­lhatâ”śâ”‚ befejezett kâ”śĂ‚r a graphics telemetryben.".into());
    }

    // The two telemetry streams have independent clocks and Graphics distance
    // can reset/wrap.  Build the Physics lap ranges from elapsed time + travelled
    // distance instead of using one global Graphics->Physics index transform.
    let physics_dist = physics_distance_cum(&physics_records);

    let mut laps = Vec::new();
    let mut prev_boundary = 0usize;
    for (lap_no, end_idx) in boundaries {
        let rr = &gfx[end_idx];

        // The first lap must also start at the real start/finish crossing,
        // not at graphics sample 0 (which may contain the pre-lap session).
        let start_idx = find_lap_start_gfx(&gfx, prev_boundary, end_idx);
        if let Some(tsec) = parse_acc_time(rr.last_time_str.trim()) {
            laps.push(Lap {
                number: lap_no,
                gfx_start: start_idx,
                gfx_end: end_idx,
                time_str: fmt_sec(tsec),
                time_sec: tsec,
                valid: rr.is_valid_lap,
            });
        }
        prev_boundary = end_idx;
    }

    let physics_lap_ranges = build_physics_lap_ranges(
        &physics_records, &physics_dist, &gfx, &laps,
    );
    if physics_lap_ranges.len() != laps.len() {
        return Err("Nem sikerült a Physics lapok megfeleltetése a Graphics lapokhoz.".into());
    }

    let best_idx = laps.iter().enumerate()
        .filter(|(_, l)| l.valid && l.time_sec > 0.0)
        .min_by(|a,b| a.1.time_sec.partial_cmp(&b.1.time_sec).unwrap())
        .map(|(i,_)| i);
    let best_sec = best_idx.map(|i| laps[i].time_sec).unwrap_or(0.0);
    let best_db = best_ever_db_path(&physics);
    let stored_best_ever = load_best_ever(&best_db, &track_name, &car_name);
    let stored_pr_sec = stored_best_ever.as_ref()
        .and_then(|v| v.get("time_sec"))
        .and_then(|v| v.as_f64());
    let new_personal_record = best_sec > 0.0
        && stored_pr_sec.map(|pr| best_sec < pr).unwrap_or(true);

    let duration = physics_records.last().map(|r| r.capture_time_sec).unwrap_or(0.0);
    let max_speed = physics_records.iter().map(|r| r.speed_kmh as f64).fold(0.0, f64::max);

    let mut lap_json = Vec::new();
    for (li, lap) in laps.iter().enumerate() {
        let range = physics_lap_ranges[li];
        let p_start = range.start;
        let p_end = range.end;
        let physics_start_time = physics_records[p_start].capture_time_sec;
        let selected: Vec<_> = physics_records[p_start..=p_end].iter().collect();
        let sectors = extract_lap_sectors(
		&gfx,
    		lap.gfx_start,
    		lap.gfx_end,
    		lap.time_sec,
	);

        let lap_distance = (gfx[lap.gfx_end].distance_traveled as f64
            - gfx[lap.gfx_start].distance_traveled as f64).abs();
        let physics_lap_distance = (physics_dist[p_end] - physics_dist[p_start]).max(0.0);

        let step = std::cmp::max(1, selected.len() / 2200);
        let telem: Vec<_> = selected.iter().step_by(step).map(|r| {
            let pi = nearest_index_by_time(&physics_records, r.capture_time_sec);
            let distance = (physics_dist[pi] - physics_dist[p_start]).clamp(0.0, physics_lap_distance);
            let progress = if physics_lap_distance > 0.0 {
                (distance / physics_lap_distance * 100.0).clamp(0.0, 100.0)
            } else { 0.0 };
            json!({
                "t": (r.capture_time_sec - physics_start_time).clamp(0.0, lap.time_sec.max(0.0)),
                "progress": progress,
                "distance": distance,
                "speed": r.speed_kmh,
                "gas": r.gas * 100.0,
                "brake": r.brake * 100.0,
                "steer": r.steer_angle,
                "gear": r.gear,
                "rpm": r.rpm
            })
        }).collect();

        let (brake_events, throttle_events) = event_points(
            &selected,
            &physics_records,
            &physics_dist,
            p_start,
            lap.time_sec,
            lap_distance.max(physics_lap_distance),
            &track_name,
        );

        let delta = if best_sec > 0.0 { lap.time_sec - best_sec } else { 0.0 };
        let mut full_throttle = 0usize;
        let mut braking = 0usize;
        let mut speed_sum = 0.0f64;
        let mut speed_max = 0.0f64;
        for r in &selected {
            let sp = r.speed_kmh as f64;
            speed_sum += sp;
            if sp > speed_max { speed_max = sp; }
            if r.gas >= 0.95 { full_throttle += 1; }
            if r.brake >= 0.05 { braking += 1; }
        }
        let sample_count = selected.len().max(1) as f64;

        lap_json.push(json!({
            "index": li,
            "lap": lap.number,
            "time": lap.time_str,
            "time_sec": lap.time_sec,
            "delta": delta,
            "valid": lap.valid,
            "avg_speed": speed_sum / sample_count,
            "max_speed": speed_max,
            "full_throttle_pct": (full_throttle as f64 / sample_count) * 100.0,
            "brake_pct": (braking as f64 / sample_count) * 100.0,
            "brake_events": brake_events,
            "throttle_events": throttle_events,
            "sectors": sectors.iter().enumerate().map(|(i, t)| json!({
                "index": i + 1,
                "time_sec": t.unwrap_or(0.0),
                "time": t.map(fmt_sec).unwrap_or_else(|| "Ă”Ă‡Ă¶".into())
            })).collect::<Vec<_>>(),
            "telem": telem
        }));
    }

    let mut best_sector_sec = vec![f64::INFINITY; 3];
    for lap in &lap_json {
        if !lap["valid"].as_bool().unwrap_or(false) { continue; }
        if let Some(sectors) = lap["sectors"].as_array() {
            for (i, sector) in sectors.iter().enumerate().take(3) {
                let t = sector["time_sec"].as_f64().unwrap_or(0.0);
                if t > 0.0 && t < best_sector_sec[i] {
                    best_sector_sec[i] = t;
                }
            }
        }
    }
    for lap in &mut lap_json {
        if let Some(sectors) = lap["sectors"].as_array_mut() {
            for (i, sector) in sectors.iter_mut().enumerate().take(3) {
                let t = sector["time_sec"].as_f64().unwrap_or(0.0);
                sector["session_best"] = json!(t > 0.0 && (t - best_sector_sec[i]).abs() < 0.0005);
            }
        }
        let idx = lap["index"].as_u64().unwrap_or(u64::MAX) as usize;
        lap["session_best"] = json!(best_idx == Some(idx));
    }

    if let Some(i) = best_idx {
        let current_best = &lap_json[i];
        let should_update = stored_best_ever.as_ref()
            .and_then(|v| v.get("time_sec"))
            .and_then(|v| v.as_f64())
            .map(|t| current_best["time_sec"].as_f64().unwrap_or(f64::INFINITY) < t)
            .unwrap_or(true);

        if should_update {
            let source_session = physics.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let candidate = json!({
                "track_name": track_name,
                "car_name": car_name,
                "lap": current_best["lap"],
                "time": current_best["time"],
                "time_sec": current_best["time_sec"],
                "valid": current_best["valid"],
                "source_session": source_session,
                "source_recording": physics.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "brake_events": current_best["brake_events"],
                "throttle_events": current_best["throttle_events"],
                "sectors": current_best["sectors"],
                "telem": current_best["telem"]
            });
            if let Err(e) = save_best_ever(&best_db, &track_name, &car_name, &candidate) {
                eprintln!("best-ever: cannot save {}: {}", best_db.display(), e);
            } else {
                eprintln!("best-ever: updated to {} from lap {}", current_best["time"], current_best["lap"]);
            }
        }
    }

    let best_ever = load_best_ever(&best_db, &track_name, &car_name)
        .or_else(|| best_idx.map(|i| {
            json!({
                "track_name": track_name,
                "car_name": car_name,
                "lap": lap_json[i]["lap"],
                "time": lap_json[i]["time"],
                "time_sec": lap_json[i]["time_sec"],
                "valid": lap_json[i]["valid"],
                "source_session": physics.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or(""),
                "source_recording": physics.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "brake_events": lap_json[i]["brake_events"],
                "throttle_events": lap_json[i]["throttle_events"],
                "telem": lap_json[i]["telem"]
            })
        }));

    let gstep = std::cmp::max(1, gfx.len() / 6000);
    let track: Vec<_> = gfx.iter().step_by(gstep).map(|r| json!({
        "x": r.car_coordinates_x, "z": r.car_coordinates_z,
        "lap": r.completed_lap, "d": r.distance_traveled
    })).collect();

    let data = json!({
        "track_name": track_name,
        "car_name": car_name,
        "physics_hz": physics_hz,
        "graphics_hz": gfx_hz,
        "physics_samples": physics_records.len(),
        "graphics_samples": gfx.len(),
        "duration": duration,
        "max_speed": max_speed,
        "best_idx": best_idx.unwrap_or(0),
        "best_time": if best_sec > 0.0 { fmt_sec(best_sec) } else { "â”śĂłĂ”Ă©ÄŚĂ”Ă‡Ĺ".to_string() },
        "best_ever": best_ever,
        "previous_pr_time_sec": stored_pr_sec,
        "new_personal_record": new_personal_record,
        "session_best_sectors": best_sector_sec.iter().map(|t| if t.is_finite() { json!({"time_sec": t, "time": fmt_sec(*t)}) } else { json!(null) }).collect::<Vec<_>>(),
        "laps": lap_json,
        "track": track
    });

    let js = serde_json::to_string(&data)?;
    let title = physics.file_stem().and_then(|s| s.to_str()).unwrap_or("ACC Session");

    let template_candidates = [
        PathBuf::from("src/report/template.html"),
        PathBuf::from("report/template.html"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("template.html")))
            .unwrap_or_else(|| PathBuf::from("template.html")),
    ];
    let template_path = template_candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .ok_or_else(|| "Nem talâ”śĂ­lhatâ”śâ”‚ a frontend template: src/report/template.html".to_string())?;
    let template = fs::read_to_string(&template_path)?;
    let html = template.replace("__JS_DATA__", &js);

    fs::write(&out, html)?;
    println!("Report: {}", out.display());
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.chars().map(|c| match c {
        '&' => "&amp;".into(),
        '<' => "&lt;".into(),
        '>' => "&gt;".into(),
        '"' => "&quot;".into(),
        '\'' => "&#39;".into(),
        _ => c.to_string(),
    }).collect()
}
