//! kzn —— GDT 引擎重写的命令行工具。
//!
//! ```text
//! kzn info  <游戏目录>
//! kzn png   <归档> <成员名> <out.png>
//! kzn run   <KIZUNAR.EXE> <ISF归档> <脚本名> [--steps N]
//! kzn dump  <KIZUNAR.EXE> <ISF归档> <输出目录> [--steps N]
//! ```

use kzn::render::{Canvas, ImageSource};
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
    let size = flag(args, "--size", 24) as f32;
    let font_path = args
        .iter()
        .position(|a| a == "--font")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".to_string());

    let font_data = std::fs::read(&font_path)?;
    let font = fontdue::Font::from_bytes(font_data.as_slice(), fontdue::FontSettings::default())
        .map_err(|e| kzn::Error(format!("字体加载失败 {}: {}", font_path, e)))?;

    let assets = Assets {
        ggd: sm2mpx::Archive::open(&format!("{}/GGD", dir))?,
        ggd2: sm2mpx::Archive::open(&format!("{}/GGD2", dir)).ok(),
    };
    // 窗框：GGD 里的 FWIN.GG2（552×152 的黑色圆角框，带外发光）
    // 窗框：FWIN2.GG2 是 60% 不透明的黑色圆角框（对话用），FWIN.GG2 是全黑版（菜单用）
    let frame = assets.image("FWIN2.GG2").or_else(|| assets.image("FWIN.GG2"));

    let exe_path = format!("{}/KIZUNAR.EXE", dir);
    let arc_path = format!("{}/ISF", dir);
    let script = load_script(&exe_path, &arc_path, script_name)?;
    let mods = load_modules(&exe_path, &arc_path)?;
    let mut vm = isf::Vm::new(&script, steps).with_modules(mods, &stem_of(script_name));
    // 只跑到第 lines 句台词就停：渲染用不着整本剧本，省一大截时间和内存
    vm.stop_texts = Some(match only_at_hint(args) {
        Some(n) => n + 1,
        None => lines,
    });
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
    let pick: Vec<usize> = match only_at {
        Some(n) => text_at.get(n).copied().into_iter().collect(),
        None => text_at.iter().take(lines).copied().collect(),
    };
    std::fs::create_dir_all(outdir)?;

    let mut player = Player::new();
    let mut written = 0usize;
    for (i, ev) in vm.events.iter().enumerate() {
        if let isf::Event::Text { speaker, text, .. } = ev {
            if !pick.contains(&i) {
                continue;
            }
            let mut cv = player.frame();
            draw_message(&mut cv, &font, frame.as_ref(), *speaker, text, size);
            if args.iter().any(|a| a == "--ascii") {
                println!("{}", cv.ascii(78, 26));
            }
            let path = format!("{}/{:03}.png", outdir, written);
            cv.write_png(&path)?;
            let (lit, tb) = cv.bright_bbox(200);
            let nl = cv
                .wrap(&font, text, size, frame_width(frame.as_ref()) - 2.0 * INSET_X)
                .len();
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

/// 图片源：GGD（.GG0）与 GGD2（.GG2）。
struct Assets {
    ggd: sm2mpx::Archive,
    ggd2: Option<sm2mpx::Archive>,
}

impl kzn::render::ImageSource for Assets {
    fn image(&self, name: &str) -> Option<ggd::Image> {
        for arc in [Some(&self.ggd), self.ggd2.as_ref()].into_iter().flatten() {
            if let Some(e) = arc.find(name).or_else(|| arc.find_stem(name)) {
                return ggd::decode(arc.bytes(e)).ok();
            }
        }
        None
    }
}

/// 文本框内边距（像素）：552×152 的框里留 26 边、18 顶。
const INSET_X: f32 = 26.0;
const INSET_Y: f32 = 18.0;
/// 行高 = 24px 字号 × 1.25。四行正好 120，加两倍 16 的上下边 = 152。
const LINE_H: f32 = 30.0;

fn only_at_hint(args: &[String]) -> Option<usize> {
    args.iter()
        .position(|a| a == "--at")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
}

fn frame_width(frame: Option<&ggd::Image>) -> f32 {
    frame.map(|f| f.width as f32).unwrap_or(552.0)
}

/// 把窗框（FWIN2.GG2，60% 黑圆角框）和台词贴到画面上。
fn draw_message(
    cv: &mut Canvas,
    font: &fontdue::Font,
    frame: Option<&ggd::Image>,
    _speaker: Option<u8>,
    text: &str,
    size: f32,
) {
    let (fw, fh) = match frame {
        Some(f) => (f.width, f.height),
        None => (552, 152),
    };
    let fx = (cv.width - fw) / 2;
    let fy = cv.height - fh - 8;
    match frame {
        Some(f) => cv.blit(f, fx as isize, fy as isize),
        None => {
            // 没有窗框时退化成半透明黑框，方便单独调文本排版
            for y in fy..fy + fh {
                for x in fx..fx + fw {
                    cv.blend(x, y, 0, 0, 0, 153);
                }
            }
        }
    }
    let inner = fw as f32 - 2.0 * INSET_X;
    let mut ty = fy as isize + INSET_Y as isize;
    for line in cv.wrap(font, text, size, inner).iter().take(4) {
        cv.draw_text(font, line, fx as isize + INSET_X as isize, ty, size, (255, 255, 255));
        ty += LINE_H as isize;
    }
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

/// 读 EXE 同目录的 KIZUNAR.SAV（真机上的存档），给虚拟机提供块 27/28。
/// 没有存档或解析失败就返回 None —— 对应引擎里「读存档失败」但继续跑的情况。
fn save_beside(exe: &str) -> Option<sav::SaveFile> {
    let dir = std::path::Path::new(exe).parent()?;
    let data = std::fs::read(dir.join("KIZUNAR.SAV")).ok()?;
    sav::parse(&data).ok()
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
                isf::Event::SaveMenu => writeln!(f, "[SAVE_MENU]")?,
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
