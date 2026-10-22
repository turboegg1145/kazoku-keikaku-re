//! kzn —— GDT 引擎重写的命令行工具。
//!
//! ```text
//! kzn info  <游戏目录>
//! kzn png   <归档> <成员名> <out.png>
//! kzn run   <KIZUNAR.EXE> <ISF归档> <脚本名> [--steps N]
//! kzn dump  <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]
//! ```

use kzn::{ggd, isf, sav, sm2mpx};
use std::io::Write;

fn usage() -> ! {
    eprintln!("{}", env!("CARGO_PKG_DESCRIPTION"));
    eprintln!();
    eprintln!("用法:");
    eprintln!("  kzn info  <游戏目录>");
    eprintln!("  kzn png   <归档> <成员名> <out.png>");
    eprintln!("  kzn run   <KIZUNAR.EXE> <ISF归档> <脚本名> [--steps N]");
    eprintln!("  kzn dump  <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]");
    eprintln!("  kzn sav   <KIZUNAR.SAV> [--roundtrip]");
    std::process::exit(2);
}

fn flag(args: &[String], name: &str, default: u64) -> u64 {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}


fn cmd_sav(args: &[String]) -> kzn::Result<()> {
    if args.is_empty() {
        usage();
    }
    let data = std::fs::read(&args[0])?;
    let sf = sav::parse(&data)?;
    println!("文件 {} 字节，块表 {} 项", data.len(), sav::TABLE_N);
    for (i, s) in sf.slots.iter().enumerate() {
        if !s.used() {
            continue;
        }
        let t = &s.time;
        let vars_used = s.vars.iter().filter(|&&v| v != 0).count();
        println!(
            "格 {:2}  {:04}-{:02}-{:02} {:02}:{:02}:{:02}  {:<24}  变量非零 {}",
            i, t[0], t[1], t[3], t[4], t[5], t[6], s.title, vars_used
        );
    }
    println!("最新存档：{:?}", sf.newest());
    println!("保留块 27：{} 字节，块 28：{} 字节", sf.system_block(27).len(), sf.system_block(28).len());
    println!("已读文本位图：{} 字节 RLE（{} 位）", sf.global_bits.len(), sav::GLOBAL_BITS_N);
    if args.iter().any(|a| a == "--roundtrip") {
        let again = sav::build(&sf);
        if again == data {
            println!("回环：逐字节一致");
        } else {
            println!("回环：不一致（{} != {} 字节）", again.len(), data.len());
            std::process::exit(1);
        }
    }
    Ok(())
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
        "shot" => cmd_shot(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "dump" => cmd_dump(&args[1..]),
        "sav" => cmd_sav(&args[1..]),
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

/// 把剧本的前几行台词渲染成 640×480 的 PNG。
///
/// 这是「引擎能出画面」的最小证明：背景 + 文本框（FWIN.GG2）+ 字形光栅化。
fn cmd_shot(args: &[String]) -> kzn::Result<()> {
    use kzn::render::Canvas;
    if args.len() < 3 {
        usage();
    }
    let dir = &args[0];
    let script_name = &args[1];
    let outdir = &args[2];
    let steps = flag(args, "--steps", 2_000_000);
    let lines = flag(args, "--lines", 8) as usize;
    let size = flag(args, "--size", 26) as f32;
    let font_path = args
        .iter()
        .position(|a| a == "--font")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".to_string());
    let bg_name = args
        .iter()
        .position(|a| a == "--bg")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let font_data = std::fs::read(&font_path)?;
    let font = fontdue::Font::from_bytes(font_data.as_slice(), fontdue::FontSettings::default())
        .map_err(|e| kzn::Error(format!("字体加载失败 {}: {}", font_path, e)))?;

    let ggd = sm2mpx::Archive::open(&format!("{}/GGD", dir))?;
    let frame_img = ggd
        .find("FWIN.GG2")
        .map(|e| ggd::decode(ggd.bytes(e)))
        .transpose()?
        .ok_or_else(|| kzn::Error("归档里没有 FWIN.GG2".into()))?;
    // 背景可能在 GGD 也可能在 GGD2，两个归档都查一下
    let ggd2 = sm2mpx::Archive::open(&format!("{}/GGD2", dir)).ok();
    // 背景：--bg 指定就固定；否则用 VM 记下来的「这句台词时的槽 0 图像」。
    let mut bg_cache: std::collections::HashMap<String, ggd::Image> = Default::default();
    if let Some(n) = &bg_name {
        if let Some(img) = load_image(&ggd, ggd2.as_ref(), n)? {
            bg_cache.insert(n.clone(), img);
        }
    }

    let exe_path = format!("{}/KIZUNAR.EXE", dir);
    let arc_path = format!("{}/ISF", dir);
    let script = load_script(&exe_path, &arc_path, script_name)?;
    let mods = load_modules(&exe_path, &arc_path)?;
    let mut vm = isf::Vm::new(&script, steps).with_modules(mods, &stem_of(script_name));
    let halt = vm.run()?;
    println!("{}: {}  steps={}", script_name, halt, vm.steps);

    let texts: Vec<(Option<u8>, String)> = vm
        .events
        .iter()
        .filter_map(|e| match e {
            isf::Event::Text { speaker, text, .. } => Some((*speaker, text.clone())),
            _ => None,
        })
        .collect();
    if texts.is_empty() {
        println!("这个脚本没有台词");
        return Ok(());
    }
    std::fs::create_dir_all(outdir)?;

    let (cw, chh) = (640usize, 480usize);
    let bx = (cw - frame_img.width) / 2;
    let by = chh - frame_img.height - 8;
    let only_at = args
        .iter()
        .position(|a| a == "--at")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());
    let mut written = 0;
    for (i, (speaker, text)) in texts.iter().enumerate().take(lines) {
        if let Some(n) = only_at {
            if i != n {
                continue;
            }
        }
        let want_bg = bg_name
            .clone()
            .or_else(|| vm.text_bg.get(i).cloned().flatten());
        if let Some(n) = &want_bg {
            if !bg_cache.contains_key(n) {
                if let Some(img) = load_image(&ggd, ggd2.as_ref(), n)? {
                    bg_cache.insert(n.clone(), img);
                }
            }
        }
        let mut cv = Canvas::new(cw, chh);
        cv.fill(0, 0, 0, 255);
        if let Some(b) = want_bg.as_ref().and_then(|n| bg_cache.get(n)) {
            cv.blit(b, 0, 0);
        }
        cv.blit(&frame_img, bx as isize, by as isize);
        let inner = frame_img.width as f32 - 60.0;
        let mut ty = by as isize + 22;
        for line in cv.wrap(&font, text, size, inner).iter().take(4) {
            cv.draw_text(&font, line, bx as isize + 30, ty, size, (255, 255, 255));
            ty += (size * 1.3) as isize;
        }
        if let Some(sp) = speaker {
            cv.draw_text(&font, &format!("S{}", sp), bx as isize + 30, by as isize - 24, 18.0, (255, 220, 160));
        }
        if args.iter().any(|a| a == "--ascii") {
            println!("{}", cv.ascii(78, 26));
        }
        let path = format!("{}/{:03}.png", outdir, i);
        cv.write_png(&path)?;
        let (_n, _bbox) = cv.stats();
        let (lit, tb) = cv.bright_bbox(200);
        let nl = cv.wrap(&font, text, size, inner).len();
        println!(
            "  {:03} 亮像素={:<6} 文字bbox={:?} 行数={}  {}",
            i,
            lit,
            tb,
            nl,
            text.chars().take(16).collect::<String>()
        );
        written += 1;
    }
    println!("写了 {} 张 -> {}", written, outdir);
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

/// 把所有 ISF 成员解密成 {模块名: Script}，供 0x02 CALLMOD 调用。
fn load_modules(
    exe: &str,
    arc: &str,
) -> kzn::Result<std::collections::HashMap<String, std::rc::Rc<isf::Script>>> {
    let exe_bytes = std::fs::read(exe)?;
    let secret = isf::find_secret(&exe_bytes)?.to_vec();
    let archive = sm2mpx::Archive::open(arc)?;
    let mut out = std::collections::HashMap::new();
    for e in archive.entries.iter() {
        let up = e.name.to_uppercase();
        if !up.ends_with(".ISF") {
            continue;
        }
        let stem = up.rsplit_once('.').map(|(a, _)| a).unwrap_or(&up).to_string();
        out.insert(stem, std::rc::Rc::new(isf::Script::parse(archive.bytes(e), &secret)?));
    }
    Ok(out)
}

/// 在 GGD / GGD2 里找一张图并解码。
fn load_image(
    ggd: &sm2mpx::Archive,
    ggd2: Option<&sm2mpx::Archive>,
    name: &str,
) -> kzn::Result<Option<ggd::Image>> {
    for arc in [Some(ggd), ggd2].into_iter().flatten() {
        if let Some(e) = arc.find(name).or_else(|| arc.find_stem(name)) {
            return Ok(Some(ggd::decode(arc.bytes(e))?));
        }
    }
    Ok(None)
}

fn stem_of(name: &str) -> String {
    name.rsplit_once('.').map(|(a, _)| a).unwrap_or(name).to_uppercase()
}

fn cmd_run(args: &[String]) -> kzn::Result<()> {
    if args.len() < 3 {
        usage();
    }
    let steps = flag(args, "--steps", 2_000_000);
    let script = load_script(&args[0], &args[1], &args[2])?;
    let mods = load_modules(&args[0], &args[1])?;
    let mut vm = isf::Vm::new(&script, steps).with_modules(mods, &stem_of(&args[2]));
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
    let mut mods: std::collections::HashMap<String, std::rc::Rc<isf::Script>> = Default::default();
    for e in archive.entries.iter() {
        let up = e.name.to_uppercase();
        if !up.ends_with(".ISF") {
            continue;
        }
        let stem = up.rsplit_once('.').map(|(a, _)| a).unwrap_or(&up).to_string();
        mods.insert(stem, std::rc::Rc::new(isf::Script::parse(archive.bytes(e), &secret)?));
    }
    let mut total_text = 0usize;
    let mut total_steps = 0u64;
    for name in &names {
        let e = archive.find(name).unwrap();
        let script = isf::Script::parse(archive.bytes(e), &secret)?;
        let mut vm = isf::Vm::new(&script, steps).with_modules(mods.clone(), &stem_of(name));
        vm.stop_on_repeat = true;
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
                isf::Event::JumpScript(v) => writeln!(f, "[JUMP] {}", v)?,
                isf::Event::Bgm { name, mode } => writeln!(f, "[BGM] {} {}", name, mode)?,
                isf::Event::BgmVolume { looping, volume } => {
                    writeln!(f, "[BGM_VOL] {} {}", *looping as u8, volume)?
                }
                isf::Event::BgmLevel(v) => writeln!(f, "[BGM_LEVEL] {}", v)?,
            }
        }
        println!("{:<14} {:<38} steps={:<8} texts={:<5}", name, halt.to_string(), vm.steps, texts);
    }
    println!();
    println!("TOTAL steps={} texts={}", total_steps, total_text);
    Ok(())
}
