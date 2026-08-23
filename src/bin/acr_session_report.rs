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
    if !sec.is_finite() || sec <= 0.0 { return "├óÔéČÔÇŁ".into(); }
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

fn physics_distance_cum(physics: &[acr_recorder::record::PhysicsRecord], hz: f64) -> Vec<f64> {
    let dt = 1.0 / hz.max(1.0);
    let mut out = Vec::with_capacity(physics.len());
    let mut d = 0.0;
    out.push(0.0);
    for i in 1..physics.len() {
        let v0 = physics[i - 1].speed_kmh.max(0.0) as f64 / 3.6;
        let v1 = physics[i].speed_kmh.max(0.0) as f64 / 3.6;
        d += 0.5 * (v0 + v1) * dt;
        out.push(d);
    }
    out
}

fn fit_line(xs: &[f64], ys: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    if n < 2.0 { return (1.0, 0.0); }
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (&x, &y) in xs.iter().zip(ys) {
        num += (x - mx) * (y - my);
        den += (x - mx) * (x - mx);
    }
    let a = if den > 1e-12 { num / den } else { 1.0 };
    (a, my - a * mx)
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

fn fit_gfx_to_physics_by_distance(
    gfx: &[acr_recorder::record::GraphicsRecord],
    physics: &[acr_recorder::record::PhysicsRecord],
    physics_hz: f64,
) -> (f64, f64) {
    let p_dist = physics_distance_cum(physics, physics_hz);
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let step = (gfx.len() / 400).max(1);
    for i in (0..gfx.len()).step_by(step) {
        let d = gfx[i].distance_traveled as f64;
        if d < 200.0 { continue; }
        let j = nearest_distance_index(&p_dist, d);
        xs.push(i as f64);
        ys.push(j as f64);
    }
    fit_line(&xs, &ys)
}

fn gfx_to_physics_index(gfx_index: usize, a: f64, b: f64, len: usize) -> usize {
    if len == 0 { return 0; }
    (a * gfx_index as f64 + b).round().clamp(0.0, (len - 1) as f64) as usize
}

fn physics_to_gfx_index(physics_index: usize, a: f64, b: f64, len: usize) -> usize {
    if len == 0 || a.abs() < 1e-12 { return 0; }
    (((physics_index as f64 - b) / a).round()).clamp(0.0, (len - 1) as f64) as usize
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
    if gfx.is_empty() || current_end <= previous_end + 1 {
        return previous_end.min(gfx.len().saturating_sub(1));
    }
    // completed_lap marks the finish of the previous lap. Start with the
    // FIRST start/finish crossing after that boundary, never the last one.
    for i in (previous_end + 1)..current_end {
        let g = &gfx[i];
        if (g.normalized_car_position as f64) <= 0.03 && !g.is_in_pit_lane {
            return i;
        }
    }
    previous_end.saturating_add(1).min(current_end)
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
    gfx: &[acr_recorder::record::GraphicsRecord],
    gfx_start: usize,
    gfx_end: usize,
    lap_start_physics_time: f64,
    lap_time_sec: f64,
    track_name: &str,
    gfx_to_phy_a: f64,
    gfx_to_phy_b: f64,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut brakes = Vec::new();
    let mut throttles = Vec::new();
    let mut prev_brake = 0.0f32;
    let mut prev_gas = 0.0f32;
    let mut last_brake_dist = -10_000.0f64;
    let mut last_throttle_dist = -10_000.0f64;

    let gfx_start_distance = gfx[gfx_start].distance_traveled as f64;

    for r in selected {
        let physics_index = nearest_index_by_time(physics, r.capture_time_sec);
        let gi = physics_to_gfx_index(physics_index, gfx_to_phy_a, gfx_to_phy_b, gfx.len())
            .clamp(gfx_start, gfx_end);
        let g = &gfx[gi];

        let progress = (g.normalized_car_position as f64 * 100.0).clamp(0.0, 100.0);
        let dist = (g.distance_traveled as f64 - gfx_start_distance).max(0.0);

        // Event time is the real physics timestamp relative to lap start.
        // Do NOT derive time from distance / lap time: that destroys braking
        // deltas whenever speed differs between the two laps.
        let event_time = (r.capture_time_sec - lap_start_physics_time)
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
        .ok_or("Haszn─é╦çlat: acr_session_report.exe <physics.rkyv> [output.html]")?;
    let out = env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        physics.parent().unwrap_or_else(|| Path::new("."))
            .join(format!("{}_report.html",
                physics.file_stem().and_then(|s| s.to_str()).unwrap_or("session")))
    });

    let gfx_path = graphics_sidecar(&physics);
    if !gfx_path.exists() {
        return Err(format!("Hi─é╦çnyzik a graphics sidecar: {}", gfx_path.display()).into());
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
        return Err("Nem tal├ílhat├│ befejezett k├Âr a graphics telemetryben.".into());
    }

    // Physics and Graphics do not share packet-id/time axes.  Align them by
    // the actual recorded distance, using the measured physics sample rate.
    // This avoids the long-session drift caused by assuming exactly 333/60 Hz.
    let physics_eff_hz = effective_physics_hz(&physics_records, &physics, physics_hz);
    let (gfx_to_phy_a, gfx_to_phy_b) = fit_gfx_to_physics_by_distance(
        &gfx, &physics_records, physics_eff_hz,
    );
    eprintln!(
        "Telemetry distance alignment: physics_eff_hz={:.3} Hz, gfx->physics index = {:.6} * gfx + {:.1}",
        physics_eff_hz, gfx_to_phy_a, gfx_to_phy_b
    );

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
        let p_start = gfx_to_physics_index(lap.gfx_start, gfx_to_phy_a, gfx_to_phy_b, physics_records.len());
        let p_end = gfx_to_physics_index(lap.gfx_end, gfx_to_phy_a, gfx_to_phy_b, physics_records.len());
        let physics_start_time = physics_records[p_start].capture_time_sec;
        let (p0, p1) = if p_start <= p_end { (p_start, p_end) } else { (p_end, p_start) };
        let selected: Vec<_> = physics_records[p0..=p1].iter().collect();
        let sectors = extract_lap_sectors(
		&gfx,
    		lap.gfx_start,
    		lap.gfx_end,
    		lap.time_sec,
	);

        // Graphics distance is the authoritative lap coordinate.
        // Do not replace it with a fixed track length.
        let gfx_start_distance = gfx[lap.gfx_start].distance_traveled as f64;

        let step = std::cmp::max(1, selected.len() / 2200);
        let telem: Vec<_> = selected.iter().step_by(step).map(|r| {
            let pi = nearest_index_by_time(&physics_records, r.capture_time_sec);
            let gi = physics_to_gfx_index(pi, gfx_to_phy_a, gfx_to_phy_b, gfx.len())
                .clamp(lap.gfx_start, lap.gfx_end);
            let g = &gfx[gi];
            let progress = (g.normalized_car_position as f64 * 100.0).clamp(0.0, 100.0);
            let distance = (g.distance_traveled as f64 - gfx_start_distance).max(0.0);
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
            &gfx,
            lap.gfx_start,
            lap.gfx_end,
            physics_start_time,
            lap.time_sec,
            &track_name,
            gfx_to_phy_a,
            gfx_to_phy_b,
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
                "time": t.map(fmt_sec).unwrap_or_else(|| "ÔÇö".into())
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
        "best_time": if best_sec > 0.0 { fmt_sec(best_sec) } else { "├óÔéČÔÇŁ".to_string() },
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
        .ok_or_else(|| "Nem tal├ílhat├│ a frontend template: src/report/template.html".to_string())?;
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
