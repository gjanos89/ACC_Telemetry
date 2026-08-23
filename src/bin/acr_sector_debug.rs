use std::env;
use std::path::Path;

use acr_recorder::export::rkyv_reader::read_graphics_rkyv;

fn parse_acc_time(s: &str) -> Option<f64> {
    let p: Vec<&str> = s.trim().split(':').collect();
    if p.len() != 3 { return None; }
    let m: f64 = p[0].parse().ok()?;
    let s: f64 = p[1].parse().ok()?;
    let ms: f64 = p[2].parse().ok()?;
    Some(m * 60.0 + s + ms / 1000.0)
}

fn fmt(v: Option<f64>) -> String {
    match v {
        Some(t) if t > 0.0 => format!("{:02}:{:06.3}", (t / 60.0).floor(), t % 60.0),
        _ => "--:--.---".to_string(),
    }
}

fn sector_time(g: &acr_recorder::record::GraphicsRecord) -> Option<f64> {
    if g.last_sector_time > 0 {
        Some(g.last_sector_time as f64 / 1000.0)
    } else {
        parse_acc_time(&g.last_sector_time_str)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or("usage: acr_sector_debug <graphics.rkyv>")?;
    let (hz, gfx) = read_graphics_rkyv(Path::new(&path))?;

    println!("graphics samples: {} @ {} Hz", gfx.len(), hz);
    println!("lap | lap time   | S1        | S2        | S3        | sum");
    println!("----+------------+-----------+-----------+-----------+-----------");

    let mut last_completed = 0i32;
    let mut lap_start = 0usize;

    for (i, g) in gfx.iter().enumerate() {
        if g.completed_lap <= last_completed || g.completed_lap <= 0 {
            continue;
        }

        let lap = g.completed_lap;
        let mut sectors = [None, None, None];
        let mut prev = gfx[lap_start].current_sector_index;

        for j in (lap_start + 1)..=i {
            let cur = gfx[j].current_sector_index;
            if cur != prev {
                let n = prev as usize;
                if n < 3 {
                    sectors[n] = sector_time(&gfx[j]);
                }
                prev = cur;
            }
        }

        let lap_time = parse_acc_time(&g.last_time_str);
        let sum = sectors.iter().flatten().sum::<f64>();

        println!(
            "{:>3} | {:>10} | {:>9} | {:>9} | {:>9} | {:>9}",
            lap,
            fmt(lap_time),
            fmt(sectors[0]),
            fmt(sectors[1]),
            fmt(sectors[2]),
            fmt(if sum > 0.0 { Some(sum) } else { None }),
        );

        last_completed = lap;
        lap_start = i + 1;
    }
}
