//! kzn —— GDT 引擎重写的命令行工具。
//!
//! ```text
//! kzn info  <游戏目录>
//! kzn png   <归档> <成员名> <out.png>
//! kzn run   <KIZUNAR.EXE> <ISF归档> <脚本名> [--steps N]
//! kzn dump  <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]
//! ```

use kzn::{ggd, isf, sm2mpx};
use std::io::Write;

fn usage() -> ! {
    eprintln!("{}", env!("CARGO_PKG_DESCRIPTION"));
    eprintln!();
    eprintln!("用法:");
    eprintln!("  kzn info  <游戏目录>");
    eprintln!("  kzn png   <归档> <成员名> <out.png>");
    eprintln!("  kzn run   <KIZUNAR.EXE> <ISF归档> <脚本名> [--steps N]");
    eprintln!("  kzn dump  <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]");
    std::process::exit(2);
}

fn flag(args: &[String], name: &str, default: u64) -> u64 {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let rc = match args[0].as_str() {
        "info" => cmd_info(&args[1..]),
        "png" => cmd_png(&args[1..]),
        "raw" => cmd_raw(&args[1..]),
        "hashes" => cmd_hashes(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "dump" => cmd_dump(&args[1..]),
        _ => usage(),
    };
    if let Err(e) = rc {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}

fn cmd_info(args: &[String]) -> kzn::Result<()> {
    let dir = args.first().map(|s| s.as_str()).unwrap_or(".");
    for (label, file) in [
        ("GGD", "GGD"),
        ("GG0/GG2", "GGD2"),
        ("ISF", "ISF"),
        ("SE", "SE"),
        ("VOICE", "VOICE1"),
        ("VOICE2", "VOICE2"),
        ("WMSC", "WMSC"),
        ("DATA", "DATA"),
    ] {
        let path = format!("{}/{}", dir, file);
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        match sm2mpx::Archive::open(&path) {
            Ok(a) => println!("{:<10} {:<10} {:>4} 个成员", label, file, a.entries.len()),
            Err(e) => println!("{:<10} {:<10} 读取失败: {}", label, file, e),
        }
    }
    Ok(())
}

fn cmd_png(args: &[String]) -> kzn::Result<()> {
    if args.len() < 3 {
        usage();
    }
    let archive = sm2mpx::Archive::open(&args[0])?;
    let data = if let Some(e) = archive.find(&args[1]) {
        archive.bytes(e).to_vec()
    } else if let Some(e) = archive.find_stem(&args[1]) {
        archive.bytes(e).to_vec()
    } else {
        return Err(kzn::Error(format!("归档里没有 {}", args[1])));
    };
    let img = ggd::decode(&data)?;
    println!(
        "{}x{}  {} 字节输入 -> {} 字节输出 ({}bpp)",
        img.width,
        img.height,
        data.len(),
        img.pixels.len(),
        img.channels * 8
    );
    ggd::write_png(&args[2], &img)?;
    println!("写入 {}", args[2]);
    Ok(())
}

/// 解出原始像素（B,G,R 或 B,G,R,A），不带 PNG 包装。对齐测试用。
fn cmd_raw(args: &[String]) -> kzn::Result<()> {
    if args.len() < 3 {
        usage();
    }
    let archive = sm2mpx::Archive::open(&args[0])?;
    let e = archive
        .find(&args[1])
        .or_else(|| archive.find_stem(&args[1]))
        .ok_or_else(|| kzn::Error(format!("归档里没有 {}", args[1])))?;
    let img = ggd::decode(archive.bytes(e))?;
    let mut out = Vec::with_capacity(16 + img.pixels.len());
    out.extend_from_slice(&(img.width as u32).to_le_bytes());
    out.extend_from_slice(&(img.height as u32).to_le_bytes());
    out.extend_from_slice(&(img.channels as u32).to_le_bytes());
    out.extend_from_slice(&(img.pixels.len() as u32).to_le_bytes());
    out.extend_from_slice(&img.pixels);
    std::fs::write(&args[2], &out)?;
    println!("{} {}x{} ch={} {} 字节 -> {}", e.name, img.width, img.height, img.channels, img.pixels.len(), args[2]);
    Ok(())
}

/// 把归档里每张图解出来，打印一行「名字 宽 高 通道 字节数 sha256(像素)」。
/// 用来和 Python 实现逐张对账。
fn cmd_hashes(args: &[String]) -> kzn::Result<()> {
    if args.is_empty() {
        usage();
    }
    let archive = sm2mpx::Archive::open(&args[0])?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    for e in &archive.entries {
        match ggd::decode(archive.bytes(e)) {
            Ok(img) => {
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(&img.pixels);
                println!(
                    "{} {} {} {} {} {:x}",
                    e.name,
                    img.width,
                    img.height,
                    img.channels,
                    img.pixels.len(),
                    digest
                );
                ok += 1;
            }
            Err(err) => {
                println!("{} FAIL {}", e.name, err);
                bad += 1;
            }
        }
    }
    eprintln!("ok={} fail={}", ok, bad);
    Ok(())
}

fn load_script(exe: &str, arc: &str, name: &str) -> kzn::Result<isf::Script> {
    let exe_bytes = std::fs::read(exe)?;
    let secret = isf::find_secret(&exe_bytes)?.to_vec();
    let archive = sm2mpx::Archive::open(arc)?;
    let e = archive
        .find(name)
        .ok_or_else(|| kzn::Error(format!("ISF 归档里没有 {}", name)))?;
    isf::Script::parse(archive.bytes(e), &secret)
}

fn cmd_run(args: &[String]) -> kzn::Result<()> {
    if args.len() < 3 {
        usage();
    }
    let steps = flag(args, "--steps", 2_000_000);
    let script = load_script(&args[0], &args[1], &args[2])?;
    let mut vm = isf::Vm::new(&script, steps);
    let halt = vm.run()?;
    let texts = vm.events.iter().filter(|e| matches!(e, isf::Event::Text { .. })).count();
    println!("{}  code {:#x}..{:#x}  labels={}", args[2], script.head_len, script.plain.len(), script.labels.len());
    println!("HALT: {}", halt);
    println!("steps={}  texts={}", vm.steps, texts);
    if !vm.unknown.is_empty() {
        let s: Vec<String> = vm
            .unknown
            .iter()
            .map(|(k, v)| format!("{:#04X} x{}", k, v))
            .collect();
        println!("skipped opcodes: {}", s.join(", "));
    }
    let mut shown = 0;
    for e in &vm.events {
        if let isf::Event::Text { speaker, text, voice } = e {
            if shown >= 15 {
                break;
            }
            shown += 1;
            let who = speaker.map(|s| format!("S{}", s)).unwrap_or_else(|| "-".into());
            println!("   {:<4} {:<10} {}", who, voice.clone().unwrap_or_else(|| "-".into()), text);
        }
    }
    Ok(())
}

fn cmd_dump(args: &[String]) -> kzn::Result<()> {
    if args.len() < 3 {
        usage();
    }
    let steps = flag(args, "--steps", 2_000_000);
    let exe_bytes = std::fs::read(&args[0])?;
    let secret = isf::find_secret(&exe_bytes)?.to_vec();
    let archive = sm2mpx::Archive::open(&args[1])?;
    std::fs::create_dir_all(&args[2])?;
    let mut names: Vec<String> = archive
        .entries
        .iter()
        .filter(|e| e.name.to_ascii_uppercase().ends_with(".ISF"))
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    let mut total_text = 0usize;
    let mut total_steps = 0u64;
    for name in &names {
        let e = archive.find(name).unwrap();
        let script = isf::Script::parse(archive.bytes(e), &secret)?;
        let mut vm = isf::Vm::new(&script, steps);
        let halt = vm.run()?;
        let texts = vm.events.iter().filter(|e| matches!(e, isf::Event::Text { .. })).count();
        total_text += texts;
        total_steps += vm.steps;
        let stem = name.rsplit_once('.').map(|(a, _)| a).unwrap_or(name);
        let path = format!("{}/{}.txt", args[2], stem);
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for ev in &vm.events {
            match ev {
                isf::Event::Text { speaker, text, voice } => {
                    let who = speaker.map(|s| format!("S{}", s)).unwrap_or_else(|| "-".into());
                    writeln!(f, "[TEXT] {:<3} {:<10} {}", who, voice.clone().unwrap_or_else(|| "-".into()), text.replace('\n', "\\n"))?;
                }
                isf::Event::Voice(v) => writeln!(f, "[VOICE] {}", v)?,
                isf::Event::Image { index, name } => writeln!(f, "[IMAGE] {} {}", index, name)?,
                isf::Event::CmdName(v) => writeln!(f, "[CMDNAME] {}", v)?,
                isf::Event::VoiceArchive(v) => writeln!(f, "[VOICE_ARC] {}", v)?,
            }
        }
        println!("{:<14} {:<38} steps={:<8} texts={:<5}", name, halt.to_string(), vm.steps, texts);
    }
    println!();
    println!("TOTAL steps={} texts={}", total_steps, total_text);
    Ok(())
}
