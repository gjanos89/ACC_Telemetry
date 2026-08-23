//! ACR Session Export v1.
//! The only component in the report pipeline that reads rkyv.
//! Usage:
//!   acr_session_export.exe <physics.rkyv> <graphics.rkyv> <output_dir>

use std::{env, error::Error, fs, fs::File, io::{BufWriter, Write}, path::{Path, PathBuf}};
use serde::Serialize;
use acr_recorder::export::rkyv_reader::{read_graphics_rkyv, read_rkyv};
use acr_recorder::record::{GraphicsRecord, PhysicsRecord};

const FORMAT: &str = "acr-session-v1";

#[derive(Serialize)]
struct SessionMeta {
    format: &'static str,
    physics_hz: u32,
    graphics_hz: u32,
    physics_records: usize,
    graphics_records: usize,
    physics_start_sec: f64,
    physics_end_sec: f64,
    graphics_start_sec: f64,
    graphics_end_sec: f64,
    physics_integrated_distance_m: f64,
    graphics_distance_max_m: f32,
    lap_count: usize,
    track_name: String,
    car_name: String,
    physics_graphics_offset_sec: f64,
}

fn q(s: &str) -> String {
    if s.contains([',','"','\n','\r']) { format!("\"{}\"", s.replace('"',"\"\"")) } else { s.to_string() }
}

fn gfx_time(g: &GraphicsRecord) -> f64 {
    g.current_time.max(0) as f64 / 1000.0
}

fn write_physics(path: &Path, p: &[PhysicsRecord]) -> Result<f64,Box<dyn Error>> {
    let mut w=BufWriter::new(File::create(path)?);
    writeln!(w,"index,capture_time_sec,elapsed_sec,packet_id,speed_kmh,steer,gas_pct,brake_pct,clutch_pct,gear,rpm,distance_integrated_m")?;
    let t0=p.first().map(|x|x.capture_time_sec).unwrap_or(0.0);
    let mut d=0.0;
    for i in 0..p.len() {
        if i>0 {
            let dt=(p[i].capture_time_sec-p[i-1].capture_time_sec).clamp(0.0,0.1);
            let v0=p[i-1].speed_kmh.max(0.0) as f64/3.6;
            let v1=p[i].speed_kmh.max(0.0) as f64/3.6;
            d += 0.5*(v0+v1)*dt;
        }
        let r=&p[i];
        writeln!(w,"{},{:.9},{:.9},{},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{:.6}",
            i,r.capture_time_sec,r.capture_time_sec-t0,r.packet_id,r.speed_kmh,r.steer_angle,
            r.gas*100.0,r.brake*100.0,r.clutch*100.0,r.gear,r.rpm,d)?;
    }
    Ok(d)
}

fn write_graphics(path:&Path,g:&[GraphicsRecord])->Result<f32,Box<dyn Error>> {
    let mut w=BufWriter::new(File::create(path)?);
    writeln!(w,"index,time_sec,packet_id,completed_lap,last_time_str,best_time_str,last_sector_time_str,is_valid_lap,is_in_pit,is_in_pit_lane,current_sector_index,distance_traveled_m,normalized_car_position")?;
    let mut maxd: f32 = 0.0;
    for (i,r) in g.iter().enumerate() {
        let t=gfx_time(r); maxd=maxd.max(r.distance_traveled);
        writeln!(w,"{},{:.9},{},{},{},{},{},{},{},{},{},{:.6},{:.9}",
            i,t,r.packet_id,r.completed_lap,q(&r.last_time_str),q(&r.best_time_str),
            q(&r.last_sector_time_str),r.is_valid_lap,r.is_in_pit,r.is_in_pit_lane,
            r.current_sector_index,r.distance_traveled,r.normalized_car_position)?;
    }
    Ok(maxd)
}

fn parse_time(s:&str)->Option<f64>{
    let mut it=s.trim().split(':');
    let a=it.next()?; let b=it.next()?;
    if it.next().is_some(){return None}
    Some(a.parse::<f64>().ok()?*60.0+b.parse::<f64>().ok()?)
}

fn write_laps(path:&Path,g:&[GraphicsRecord])->Result<usize,Box<dyn Error>>{
    let mut w=BufWriter::new(File::create(path)?);
    writeln!(w,"lap,start_index,end_index,start_time_sec,end_time_sec,lap_time_sec,start_distance_m,end_distance_m,lap_distance_m,valid")?;
    let mut bounds:Vec<(i32,usize)>=Vec::new();
    let mut prev=g.first().map(|x|x.completed_lap).unwrap_or(0);
    for i in 1..g.len() {
        let cur=g[i].completed_lap;
        if cur>prev { bounds.push((cur,i)); prev=cur; }
    }
    let mut count=0usize;
    let mut start_idx=0usize;
    for (lap,e) in &bounds {
        if *e<=start_idx { continue; }
        let gs=&g[start_idx]; let ge=&g[*e];
        let st=gfx_time(gs); let et=gfx_time(ge);
        let lt=parse_time(&ge.last_time_str).unwrap_or(et-st);
        writeln!(w,"{},{},{},{:.9},{:.9},{:.6},{:.6},{:.6},{:.6},{}",
            lap,start_idx,e,st,et,lt,gs.distance_traveled,ge.distance_traveled,
            (ge.distance_traveled-gs.distance_traveled).abs(),ge.is_valid_lap)?;
        count+=1;
        start_idx=*e;
    }
    Ok(count)
}

fn main()->Result<(),Box<dyn Error>>{
    let mut a=env::args().skip(1);
    let physics=PathBuf::from(a.next().ok_or("usage: acr_session_export.exe <physics.rkyv> <graphics.rkyv> <output_dir>")?);
    let graphics=PathBuf::from(a.next().ok_or("missing graphics.rkyv")?);
    let out=PathBuf::from(a.next().ok_or("missing output_dir")?);
    fs::create_dir_all(&out)?;
    let (phz,p)=read_rkyv(&physics)?;
    let (ghz,g)=read_graphics_rkyv(&graphics)?;
    if p.is_empty()||g.is_empty(){return Err("empty telemetry".into());}
    let stem=physics.file_stem().and_then(|x|x.to_str()).unwrap_or("session");
    let pc=out.join(format!("{stem}.physics.csv"));
    let gc=out.join(format!("{stem}.graphics.csv"));
    let lc=out.join(format!("{stem}.laps.csv"));
    let sj=out.join(format!("{stem}.session.json"));
    let pd=write_physics(&pc,&p)?;
    let gd=write_graphics(&gc,&g)?;
    let laps=write_laps(&lc,&g)?;
    let p0=p.first().unwrap().capture_time_sec;
    let p1=p.last().unwrap().capture_time_sec;
    let g0=gfx_time(g.first().unwrap());
    let g1=gfx_time(g.last().unwrap());
    let meta=SessionMeta{
        format:FORMAT,physics_hz:phz,graphics_hz:ghz,physics_records:p.len(),graphics_records:g.len(),
        physics_start_sec:p0,physics_end_sec:p1,graphics_start_sec:g0,graphics_end_sec:g1,
        physics_integrated_distance_m:pd,graphics_distance_max_m:gd,lap_count:laps,
        track_name:"".into(),car_name:"".into(),
        physics_graphics_offset_sec:(p0 - g0)
    };
    fs::write(&sj,serde_json::to_string_pretty(&meta)?)?;
    println!("ACR SESSION EXPORT v1");
    println!("Physics records : {}",p.len());
    println!("Graphics records: {}",g.len());
    println!("Lap intervals   : {}",laps);
    println!("Physics distance: {:.3} m",pd);
    println!("Graphics max    : {:.3} m",gd);
    println!("Export directory: {}",out.display());
    Ok(())
}
