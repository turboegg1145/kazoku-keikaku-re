//! kzn —— GDT 引擎重写的命令行工具。
//!
//! ```text
//! kzn info  <游戏目录>
//! kzn png   <归档> <成员名> <out.png>
//! kzn run   <KIZUNAR.EXE> <ISF归档> <脚本名> [--steps N]
//! kzn dump  <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]
//! ```

use kzn::render::Canvas;
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
    eprintln!("  kzn text  <游戏目录> <脚本名> [--lines N]   台词排版（位置/分屏/等待）");
    eprintln!("  kzn abi   <游戏目录> <脚本名> [--frames N]  走 C ABI 跑一段（iOS 同款路径）");
    std::process::exit(2);
}

fn hexs(v: &[u8]) -> String {
    v.iter().map(|b| format!("{:02x}", b)).collect()
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
        "audio" => cmd_audio(&args[1..]),
        "play" => cmd_play(&args[1..]),
        "mask" => cmd_mask(&args[1..]),
        "text" => cmd_text(&args[1..]),
        "abi" => cmd_abi(&args[1..]),
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
        archive.bytes(e)?
    } else if let Some(e) = archive.find_stem(&args[1]) {
        archive.bytes(e)?
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
    let img = ggd::decode(&archive.bytes(e)?)?;
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
        match ggd::decode(&archive.bytes(e)?) {
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

/// 把剧本的前几句台词渲染成 640×480 的 PNG。
///
/// 画面来自 Player：它照引擎的 0x50/0x51/0x56/0x57 指令流维护 128 块面，
/// 0 号面就是屏幕。台词本身按 0x2B 的文本 + FWIN.GG2 的窗框合成。
fn cmd_shot(args: &[String]) -> kzn::Result<()> {
    use kzn::render::Player;
    if args.len() < 3 {
        usage();
    }
    let dir = &args[0];
    let script_name = &args[1];
    let outdir = &args[2];
    let steps = flag(args, "--steps", 2_000_000);
    let lines = flag(args, "--lines", 8) as usize;
    let size = flag(args, "--size", 20) as f32;
    let font_path = args
        .iter()
        .position(|a| a == "--font")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".to_string());

    let font_data = std::fs::read(&font_path)?;
    let assets = kzn::assets::Assets::open(dir, false)?;
    // 窗框：FWIN2.GG2 是 60% 不透明的黑色圆角框（对话用），FWIN.GG2 是全黑版（菜单用）
    let composer = kzn::compose::Composer::from_bytes(font_data, size, &assets)?;

    let exe_path = format!("{}/KIZUNAR.EXE", dir);
    let arc_path = format!("{}/ISF", dir);
    let script = load_script(&exe_path, &arc_path, script_name)?;
    let mods = load_modules(&exe_path, &arc_path)?;
    let mut vm = isf::Vm::new(&script, steps).with_modules(mods, &stem_of(script_name));
    // 只跑到需要的那一句就停：渲染用不着整本剧本，省一大截时间和内存
    let choice_at = args
        .iter()
        .position(|a| a == "--choice")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());
    match choice_at {
        Some(n) => vm.stop_choices = Some(n + 1),
        None => {
            vm.stop_texts = Some(match only_at_hint(args) {
                Some(n) => n + 1,
                None => lines,
            })
        }
    }
    let halt = vm.run()?;
    println!("{}: {}  steps={}", script_name, halt, vm.steps);

    // 第 k 句台词在事件流里的下标
    let text_at: Vec<usize> = vm
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, isf::Event::Text { .. }))
        .map(|(i, _)| i)
        .collect();
    if text_at.is_empty() {
        println!("这个脚本没有台词");
        return Ok(());
    }
    let only_at = only_at_hint(args);
    let pick: Vec<usize> = if choice_at.is_some() {
        Vec::new()
    } else {
        match only_at {
            Some(n) => text_at.get(n).copied().into_iter().collect(),
            None => text_at.iter().take(lines).copied().collect(),
        }
    };
    std::fs::create_dir_all(outdir)?;

    // --choice N：在第 N 个选项窗口刚打开时截一张（画面 + 选项文字）
    let shot_at = choice_at.map(|_| {
        vm.events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, isf::Event::ChoiceOpen { .. }))
            .map(|(i, _)| i)
            .last()
            .unwrap_or(usize::MAX)
    });
    let mut player = Player::new();
    let mut written = 0usize;
    for (i, ev) in vm.events.iter().enumerate() {
        if Some(i) == shot_at {
            player.apply(ev, &assets);
            let cv = composer.compose(&player, None, None);
            if args.iter().any(|a| a == "--ascii") {
                println!("{}", cv.ascii(78, 26));
            }
            let path = format!("{}/{:03}.png", outdir, written);
            cv.write_png(&path)?;
            println!("  选项窗口 -> {}", path);
            written += 1;
            continue;
        }
        if let isf::Event::Text { text, raw, .. } = ev {
            if !pick.contains(&i) {
                continue;
            }
            let cv = composer.compose(&player, Some(raw), None);
            if args.iter().any(|a| a == "--ascii") {
                println!("{}", cv.ascii(78, 26));
            }
            let path = format!("{}/{:03}.png", outdir, written);
            cv.write_png(&path)?;
            let (lit, tb) = cv.bright_bbox(200);
            let nl: usize = composer
                .layout(raw, &kzn::text::Metrics::default())
                .pages
                .iter()
                .map(|p| p.rows.len())
                .sum();
            println!(
                "  {:03} 亮像素={:<6} 文字bbox={:?} 行数={}  {}",
                written,
                lit,
                tb,
                nl,
                text.chars().take(16).collect::<String>()
            );
            written += 1;
            continue;
        }
        player.apply(ev, &assets);
    }
    if !player.missing.is_empty() {
        println!("归档里找不到的图 {} 张: {:?}", player.missing.len(), &player.missing[..player.missing.len().min(8)]);
    }
    println!(
        "绘制 {} 次，cmd 分布 {:?}",
        player.draws,
        player.cmds.iter().map(|(k, v)| (format!("{:#04x}", k), *v)).collect::<Vec<_>>()
    );
    println!("写了 {} 张 -> {}", written, outdir);
    Ok(())
}


fn only_at_hint(args: &[String]) -> Option<usize> {
    args.iter()
        .position(|a| a == "--at")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
}

fn load_script(exe: &str, arc: &str, name: &str) -> kzn::Result<isf::Script> {
    let exe_bytes = std::fs::read(exe)?;
    let secret = isf::find_secret(&exe_bytes)?.to_vec();
    let archive = sm2mpx::Archive::open(arc)?;
    let e = archive
        .find(name)
        .ok_or_else(|| kzn::Error(format!("ISF 归档里没有 {}", name)))?;
    isf::Script::parse(&archive.bytes(e)?, &secret)
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
        out.insert(stem, std::rc::Rc::new(isf::Script::parse(&archive.bytes(e)?, &secret)?));
    }
    Ok(out)
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
        if let isf::Event::Text { speaker, text, voice, .. } = e {
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

/// 读 EXE 同目录的 KIZUNAR.SAV（真机上的存档），给虚拟机提供块 27/28。
/// 没有存档或解析失败就返回 None —— 对应引擎里「读存档失败」但继续跑的情况。
fn save_beside(exe: &str) -> Option<sav::SaveFile> {
    let dir = std::path::Path::new(exe).parent()?;
    let data = std::fs::read(dir.join("KIZUNAR.SAV")).ok()?;
    sav::parse(&data).ok()
}

/// kzn audio <归档> <成员名> [--wav 输出路径] [--loop]
/// 解码一段 Ogg Vorbis 并报告时长/峰值；给音频链路做验证用。
fn cmd_audio(args: &[String]) -> kzn::Result<()> {
    if args.len() < 2 {
        usage();
    }
    let archive = sm2mpx::Archive::open(&args[0])?;
    let want = &args[1];
    let e = archive
        .find(want)
        .or_else(|| archive.find_stem(want))
        .ok_or_else(|| format!("归档里没有 {}", want))?;
    let bytes = archive.bytes(e)?;
    let t = kzn::audio::decode_ogg(&bytes)?;
    let peak = t.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    println!(
        "{}: {:.2} 秒  {} Hz  {} 声道  峰值 {}",
        want,
        t.duration(),
        t.rate,
        t.channels,
        peak
    );
    if let Some(i) = args.iter().position(|a| a == "--wav") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "out.wav".to_string());
        kzn::audio::write_wav(&out, &t.samples, t.rate, t.channels)?;
        println!("  -> {}", out);
    }
    Ok(())
}

/// `kzn play <目录> <剧本> [--lines N] [--steps N] [--no-audio] [--budget N]`
///
/// 无人值守跑一遍：台词全部瞬间显示、选项永远选第 0 项。
/// 这是主循环的端到端验证——能把一个剧本从头跑到尾，就说明等待/推进/事件分发都对上了。
/// `kzn mask <目录> <蒙版名> [--png out.png]`
/// 解一张 0x60 特效蒙版（DATA 里的 EFF*/SEF*.BIN，320×240 8bpp BMP），
/// 打印直方图，可选导出成灰度 PNG。
fn cmd_mask(args: &[String]) -> kzn::Result<()> {
    if args.len() < 2 {
        usage();
    }
    let arc = sm2mpx::Archive::open(&format!("{}/DATA", &args[0]))?;
    let want = &args[1];
    let e = arc
        .find(want)
        .or_else(|| arc.find_stem(want))
        .ok_or_else(|| format!("DATA 里没有 {}", want))?;
    let bytes = arc.bytes(e)?;
    let (w, h, idx) = kzn::render::decode_mask(&bytes)
        .ok_or_else(|| format!("{} 不是 8bpp BMP", want))?;
    let mut hist = [0u64; 256];
    for &v in &idx {
        hist[v as usize] += 1;
    }
    let nz = idx.iter().filter(|&&v| v != 0).count();
    let full = idx.iter().filter(|&&v| v == 255).count();
    println!("{}: {}x{}  非零 {} ({:.1}%)  全不透明 {} ({:.1}%)",
        want, w, h, nz, nz as f64 * 100.0 / idx.len() as f64, full, full as f64 * 100.0 / idx.len() as f64);
    let mut peaks: Vec<(usize, u64)> = hist.iter().copied().enumerate().collect();
    peaks.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
    println!("  最多的灰度: {:?}", &peaks[..5]);
    if let Some(i) = args.iter().position(|a| a == "--png") {
        let out = args.get(i + 1).cloned().unwrap_or_else(|| "mask.png".to_string());
        let mut cv = Canvas::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = idx[y * w + x];
                let d = (y * w + x) * 4;
                cv.pixels[d] = v;
                cv.pixels[d + 1] = v;
                cv.pixels[d + 2] = v;
                cv.pixels[d + 3] = 255;
            }
        }
        cv.write_png(&out)?;
        println!("  -> {}", out);
    }
    Ok(())
}

fn cmd_play(args: &[String]) -> kzn::Result<()> {
    if args.len() < 2 {
        usage();
    }
    let dir = &args[0];
    let script_name = &args[1];
    let steps = flag(args, "--steps", 5_000_000);
    let want_lines = flag(args, "--lines", 40) as usize;
    let budget = flag(args, "--budget", 4_000) as usize;
    let audio = !args.iter().any(|a| a == "--no-audio");
    let verbose = !args.iter().any(|a| a == "--quiet");

    let assets = kzn::assets::Assets::open(dir, audio)?;

    let exe_path = format!("{}/KIZUNAR.EXE", dir);
    let arc_path = format!("{}/ISF", dir);
    let script = load_script(&exe_path, &arc_path, script_name)?;
    let mods = load_modules(&exe_path, &arc_path)?;
    let vm = isf::Vm::new(&script, steps).with_modules(mods, &stem_of(script_name));

    let mut eng = kzn::engine::Engine::new(vm).with_dir(dir);
    eng.pump(&assets, &assets, budget);

    let mut printed = 0usize;
    let mut shown_choices = 0usize;
    let mut frames = 0u64;
    loop {
        if verbose {
            while printed < eng.log.len() {
                let l = &eng.log[printed];
                let who = match l.speaker {
                    Some(s) => format!("#{:02X}", s),
                    None => "--".to_string(),
                };
                println!("{} [{}] {}", printed + 1, who, l.text);
                printed += 1;
            }
            while shown_choices < eng.choice_log.len() {
                let (w, opts) = &eng.choice_log[shown_choices];
                println!("  选项窗 {}: {}", w, opts.join(" / "));
                shown_choices += 1;
            }
        }
        if eng.lines >= want_lines {
            println!("-- 已跑 {} 句台词，主动停", eng.lines);
            break;
        }
        if !eng.auto_advance(&assets, &assets) {
            break;
        }
        if args.iter().any(|a| a == "--trace") && frames % 20 == 0 {
            println!("  .. 帧 {} pc={:#x} 台词 {} 等待 {:?}", frames, eng.vm.code, eng.lines, eng.wait);
        }
        eng.tick(1000.0 / 60.0);
        frames += 1;
        if frames > 5_000_000 {
            println!("-- 帧数上限，强制停");
            break;
        }
    }

    println!(
        "{}: 台词 {} 句，选项 {} 个，帧 {}，指令 {}，结束 {:?}",
        script_name, eng.lines, eng.choices, frames, eng.vm.steps, eng.wait
    );
    if args.iter().any(|a| a == "--trace") {
        let tail: Vec<String> = eng
            .vm
            .recent
            .iter()
            .map(|(a, o)| format!("{:#x}:{:02x}", a, o))
            .collect();
        println!("  最后 64 条: {}", tail.join(" "));
    }
    if eng.player.shakes > 0 {
        println!("画面抖动 {} 次", eng.player.shakes);
    }
    if !eng.player.effects.is_empty() {
        let mut uniq: Vec<&String> = Vec::new();
        for e in &eng.player.effects {
            if !uniq.iter().any(|u| *u == e) {
                uniq.push(e);
            }
        }
        println!("特效 {} 次: {:?}", eng.player.effects.len(), uniq);
    }
    if !eng.player.missing.is_empty() {
        println!("归档里找不到的图: {:?}", eng.player.missing);
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
        mods.insert(stem, std::rc::Rc::new(isf::Script::parse(&archive.bytes(e)?, &secret)?));
    }
    let mut total_text = 0usize;
    let mut total_steps = 0u64;
    for name in &names {
        let e = archive.find(name).unwrap();
        let script = isf::Script::parse(&archive.bytes(e)?, &secret)?;
        let mut vm = isf::Vm::new(&script, steps).with_modules(mods.clone(), &stem_of(name));
        if let Some(sf) = &save_beside(&args[0]) {
            vm.load_system(sf);
        }
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
                isf::Event::Text { speaker, text, voice, .. } => {
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
                isf::Event::Raw13(v) => writeln!(f, "[RAW13] {}", hexs(v))?,
                isf::Event::SurfaceParam { surface, a, b, c, d, flag } => {
                    writeln!(f, "[SURF_PARAM] surf={} {},{},{},{} flag={}", surface, a, b, c, d, flag)?
                }
                isf::Event::SurfaceImage { surface, mode, names } => {
                    writeln!(f, "[SURF_IMG] surf={} mode={} names={}", surface, mode, names.join("|"))?
                }
                isf::Event::ObjParam { index, a, b, flag } => {
                    writeln!(f, "[OBJ] idx={} {} {} flag={}", index, a, b, flag)?
                }
                isf::Event::SurfaceText { surface, flag, text } => {
                    writeln!(f, "[SURF_TEXT] surf={} flag={} text={}", surface, flag, text)?
                }
                isf::Event::SurfaceFlag { surface, on } => {
                    writeln!(f, "[SURF_FLAG] surf={} on={}", surface, on)?
                }
                isf::Event::Surface27 { surface, value } => {
                    writeln!(f, "[SURF27] surf={} value={}", surface, value)?
                }
                isf::Event::Surface29(s) => writeln!(f, "[SURF29] surf={}", s)?,
                isf::Event::Surface2A(s) => writeln!(f, "[SURF2A] surf={}", s)?,
                isf::Event::BgmStop => writeln!(f, "[BGM_STOP]")?,
                isf::Event::VoicePlay(v) => writeln!(f, "[VOICE_PLAY] {}", v)?,
                isf::Event::SelectIndex(i) => writeln!(f, "[SELECT] {}", i)?,
                isf::Event::TextBuf(t) => writeln!(f, "[TEXT_BUF] {}", t)?,
                isf::Event::BitsFill { from, to, value } => {
                    writeln!(f, "[BITS_FILL] {}..{} = {}", from, to, value)?
                }
                isf::Event::SysFunc { kind, arg } => {
                    writeln!(f, "[SYSFUNC] kind={} arg={}", kind, arg)?
                }
                isf::Event::VarsFill { from, to, value } => {
                    writeln!(f, "[VARS_FILL] {}..{} = {}", from, to, value)?
                }
                isf::Event::SaveMenu { bits, vars } => {
                    writeln!(f, "[SAVE_MENU] bits={} vars={}", bits, vars)?
                }
                isf::Event::SysCond { arg, kind } => {
                    writeln!(f, "[SYSCOND] kind={} arg={}", kind, arg)?
                }
                isf::Event::UiText { surface, name } => {
                    writeln!(f, "[UI_TEXT] surf={} name={}", surface, name)?
                }
                isf::Event::InputReset => writeln!(f, "[INPUT_RESET]")?,
                isf::Event::Nop87 => writeln!(f, "[NOP87]")?,
                isf::Event::CondGroup { index, slots } => {
                    let list: Vec<String> = slots.iter().map(|v| v.to_string()).collect();
                    writeln!(f, "[COND_GROUP] idx={} slots={}", index, list.join(","))?
                }
                isf::Event::CondGroupClear => writeln!(f, "[COND_CLEAR]")?,
                isf::Event::SaveMenuDraw => writeln!(f, "[SAVE_MENU_DRAW]")?,
                isf::Event::Color6(v) => writeln!(f, "[COLOR6] {}", hexs(v))?,
                isf::Event::WindowText { surface, flag, text } => {
                    writeln!(f, "[WIN_TEXT] surf={} flag={} text={}", surface, flag, text)?
                }
                isf::Event::TextObjNew { index, text } => {
                    writeln!(f, "[TEXT_OBJ] idx={} text={}", index, text)?
                }
                isf::Event::TextObjLine { index, line, text } => {
                    writeln!(f, "[TEXT_OBJ_LINE] idx={} line={} text={}", index, line, text)?
                }
                isf::Event::TextObjCell { index, line, col, value, text } => {
                    writeln!(f, "[TEXT_OBJ_CELL] idx={} line={} col={} value={} text={}",
                             index, line, col, value, text)?
                }
                isf::Event::TextObjFull(v) => writeln!(f, "[TEXT_OBJ_FULL] {}", hexs(v))?,
                isf::Event::TextObjFlag(v) => writeln!(f, "[TEXT_OBJ_FLAG] {}", v)?,
                isf::Event::Movie { x, y, w, h, name } => {
                    writeln!(f, "[MOVIE] {},{} {}x{} {}", x, y, w, h, name)?
                }
                isf::Event::Deadline(ms) => writeln!(f, "[DEADLINE] {}", ms)?,
                isf::Event::NopF2 => writeln!(f, "[NOP_F2]")?,
                isf::Event::FuncFa(v) => writeln!(f, "[FUNC_FA] {}", v)?,
                isf::Event::BufCopy => writeln!(f, "[BUF_COPY]")?,
                isf::Event::ColorChannels { surface, mask } => {
                    writeln!(f, "[COLOR_CH] surf={} mask={}", surface, mask)?
                }
                isf::Event::Color5(v) => writeln!(f, "[COLOR5] {}", hexs(v))?,
                isf::Event::Effect { args, name } => {
                    let list: Vec<String> = args.iter().map(|v| v.to_string()).collect();
                    writeln!(f, "[EFFECT] {} name={}", list.join(","), name)?
                }
                isf::Event::Transition { kind, a, b, dur } => {
                    writeln!(f, "[TRANS] kind={} {}/{} dur={}", kind, a, b, dur)?
                }
                isf::Event::Choice { window, index, x, y, w, h, text } => {
                    writeln!(f, "[CHOICE] win={} idx={} rect={},{},{},{} text={}", window, index, x, y, w, h, text)?
                }
                isf::Event::ChoiceOpen { window, count, flags } => {
                    writeln!(f, "[CHOICE_OPEN] win={} count={} flags={}", window, count, flags)?
                }
                isf::Event::ChoiceClose { window } => {
                    writeln!(f, "[CHOICE_CLOSE] win={}", window)?
                }
                isf::Event::ChoiceInput { window, value, flags } => {
                    writeln!(f, "[CHOICE_INPUT] win={} value={} flags={}", window, value, flags)?
                }
                isf::Event::ChoiceClear => writeln!(f, "[CHOICE_CLEAR]")?,
                isf::Event::ChoiceShuffle => writeln!(f, "[CHOICE_SHUFFLE]")?,
                isf::Event::SeLoad { name, slot } => {
                    writeln!(f, "[SE_LOAD] slot={} name={}", slot, name)?
                }
                isf::Event::SePlay { slot, param } => {
                    writeln!(f, "[SE_PLAY] slot={} param={}", slot, param)?
                }
                isf::Event::SeStop { slot } => writeln!(f, "[SE_STOP] slot={}", slot)?,
                isf::Event::VoiceParam { value } => writeln!(f, "[VOICE_PARAM] {}", value)?,
                isf::Event::SeRepeat { slot, value } => {
                    writeln!(f, "[SE_REPEAT] slot={} value={}", slot, value)?
                }
                isf::Event::FlagFromVoice { which, index } => {
                    writeln!(f, "[FLAG_VOICE] which={} idx={}", which, index)?
                }
                isf::Event::UiMode(m) => writeln!(f, "[UI_MODE] {}", m)?,
                isf::Event::GlobalSave { start, count, x, which } => {
                    writeln!(f, "[GSAVE] which={} start={} count={} x={}", which, start, count, x)?
                }
                isf::Event::GlobalLoad { start, count, x, which } => {
                    writeln!(f, "[GLOAD] which={} start={} count={} x={}", which, start, count, x)?
                }
                isf::Event::Surface { index, w, h } => {
                    writeln!(f, "[SURF] idx={} {}x{}", index, w, h)?
                }
                isf::Event::SurfaceRect { index, x, y } => {
                    writeln!(f, "[SURF_POS] idx={} {},{}", index, x, y)?
                }
                isf::Event::WindowRect { surface, r, g, b } => {
                    writeln!(f, "[WIN_COLOR] surf={} rgb={:02x}{:02x}{:02x}", surface, r, g, b)?
                }
                isf::Event::WindowFx { value, r, g, b, flag } => {
                    writeln!(f, "[WIN_FX] value={} rgb={:02x}{:02x}{:02x} flag={}", value, r, g, b, flag)?
                }
                isf::Event::Draw { cmd, img, x, y, w, h, dur, dx, dy, extra } => {
                    let mut line = format!(
                        "[DRAW] cmd={} img={} src={},{},{},{} dur={} dst={},{}",
                        cmd, img, x, y, w, h, dur, dx, dy
                    );
                    for v in extra {
                        line.push_str(&format!(" +{}", v));
                    }
                    writeln!(f, "{}", line)?;
                }
            }
        }
        println!("{:<14} {:<38} steps={:<8} texts={:<5}", name, halt.to_string(), vm.steps, texts);
    }
    println!();
    println!("TOTAL steps={} texts={}", total_steps, total_text);
    Ok(())
}

/// `kzn text`：把台词的排版结果打出来（位置/分屏/等待次数）。
fn cmd_text(args: &[String]) -> kzn::Result<()> {
    use kzn::text::{default_measure, parse, Metrics};
    if args.len() < 3 {
        usage();
    }
    let dir = &args[0];
    let script_name = &args[1];
    let want = flag(args, "--lines", 20) as usize;
    let steps = flag(args, "--steps", 2_000_000);
    let exe_path = format!("{}/KIZUNAR.EXE", dir);
    let arc_path = format!("{}/ISF", dir);
    let script = load_script(&exe_path, &arc_path, script_name)?;
    let mods = load_modules(&exe_path, &arc_path)?;
    let mut vm = isf::Vm::new(&script, steps).with_modules(mods, &stem_of(script_name));
    let _ = vm.run();
    let m = Metrics::default();
    let mut n = 0usize;
    let mut total_waits = 0usize;
    let mut max_rows = 0usize;
    for e in &vm.events {
        if let isf::Event::Text { speaker, raw, .. } = e {
            n += 1;
            let l = parse(raw, &m, default_measure);
            total_waits += l.waits;
            let rows: usize = l.pages.iter().map(|p| p.rows.len()).sum();
            max_rows = max_rows.max(rows);
            println!(
                "[{:03}] 说话人 {:?} 字号 {} 屏 {} 行 {} 等待 {} 颜色 {:06X}/{:06X}/{:06X}{}",
                n, speaker, l.size, l.pages.len(), rows, l.waits,
                l.colors[0], l.colors[1], l.colors[2],
                if l.name.is_empty() { String::new() } else { format!(" 名字「{}」", l.name) }
            );
            for (pi, p) in l.pages.iter().enumerate() {
                for r in &p.rows {
                    println!("   p{} y={:<3} x={:<3} {}", pi, r.y, r.x, r.text);
                }
                if p.wait {
                    println!("   p{} —— 等待点击", pi);
                }
            }
            if n >= want {
                break;
            }
        }
    }
    println!("共 {} 句，等待 {} 次，单句最多 {} 行", n, total_waits, max_rows);
    Ok(())
}

/// `kzn abi <目录> <剧本> [--frames N] [--font 路径] [--out 文件]`
///
/// 完全走 C ABI（iOS 端就是这条路径）跑一段，验证接口可用。
fn cmd_abi(args: &[String]) -> kzn::Result<()> {
    use kzn::capi as c;
    use std::ffi::CString;
    if args.len() < 2 {
        usage();
    }
    let dir = CString::new(args[0].as_str()).map_err(|_| kzn::Error("目录里有 NUL".into()))?;
    let script = CString::new(args[1].as_str()).map_err(|_| kzn::Error("名字里有 NUL".into()))?;
    let font_path = args
        .iter()
        .position(|a| a == "--font")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".to_string());
    let font = CString::new(font_path).map_err(|_| kzn::Error("字体路径有 NUL".into()))?;
    let frames = flag(args, "--frames", 600) as usize;
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/tmp/kzn_abi.png".to_string());

    let k = c::kzn_open(dir.as_ptr(), 1, font.as_ptr());
    if k.is_null() {
        return Err(kzn::Error("kzn_open 失败（目录/字体/归档）".into()));
    }
    if c::kzn_start(k, script.as_ptr()) != 0 {
        let mut buf = vec![0u8; 512];
        c::kzn_error(k, buf.as_mut_ptr() as *mut _, buf.len() as i32);
        let msg = String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string();
        c::kzn_free(k);
        return Err(kzn::Error(format!("kzn_start 失败: {}", msg)));
    }

    let mut rgba = vec![0u8; 640 * 480 * 4];
    let mut pcm = vec![0i16; 2048];
    let mut clicks = 0u32;
    let mut last = String::new();
    let mut peak = 0i16;
    for _ in 0..frames {
        c::kzn_tick(k, 16.0);
        c::kzn_pump(k, 4000);
        let n = c::kzn_audio(k, pcm.as_mut_ptr(), 1024, 44100);
        for &s in &pcm[..n.max(0) as usize] {
            peak = peak.max(s.abs());
        }
        match c::kzn_state(k) {
            c::KZN_CHOICE => {
                let mut r = [0i32; 4];
                if c::kzn_choice_rect(k, 0, 0, r.as_mut_ptr()) == 1 {
                    c::kzn_click(k, r[0] + r[2] / 2, r[1] + r[3] / 2);
                    clicks += 1;
                }
            }
            c::KZN_TEXT | c::KZN_INPUT => {
                c::kzn_click(k, 300, 400);
                clicks += 1;
            }
            c::KZN_HALT | c::KZN_FAILED => break,
            _ => {}
        }
        let mut tb = vec![0u8; 4096];
        let n = c::kzn_text(k, tb.as_mut_ptr() as *mut _, tb.len() as i32);
        if n > 0 {
            last = String::from_utf8_lossy(&tb[..n as usize]).to_string();
        }
    }
    let n = c::kzn_frame(k, rgba.as_mut_ptr());
    let mut cv = Canvas::new(640, 480);
    cv.pixels.copy_from_slice(&rgba[..n as usize]);
    cv.write_png(&out)?;
    let (lit, bbox) = cv.bright_bbox(200);
    println!("ABI 跑完 {} 帧：状态 {}，点击 {} 次，音频峰值 {}", frames, c::kzn_state(k), clicks, peak);
    println!("  最后一句话: {}", last);
    println!("  帧 -> {}（亮像素 {}，bbox {:?}）", out, lit, bbox);
    c::kzn_free(k);
    Ok(())
}