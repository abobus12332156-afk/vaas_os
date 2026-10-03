#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use bootloader::BootInfo;
use core::panic::PanicInfo;
use x86_64::VirtAddr;

extern crate alloc;

mod interrupts;
mod vga_buffer;

pub mod allocator;
pub mod gdt;
pub mod memory;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n--- KERNEL PANIC ---");
    println!("{}", info);
    loop {
        x86_64::instructions::hlt();
    }
}

fn print_cpu_info() {
    use core::arch::x86_64::__cpuid;

    let vendor_leaf = unsafe { __cpuid(0) };
    let mut vendor_bytes = [0_u8; 12];
    vendor_bytes[0..4].copy_from_slice(&vendor_leaf.ebx.to_le_bytes());
    vendor_bytes[4..8].copy_from_slice(&vendor_leaf.edx.to_le_bytes());
    vendor_bytes[8..12].copy_from_slice(&vendor_leaf.ecx.to_le_bytes());

    let vendor = core::str::from_utf8(&vendor_bytes).unwrap();
    println!("CPU Vendor: {}", vendor);

    let max_extended_leaf = unsafe { __cpuid(0x8000_0000) }.eax;
    if max_extended_leaf < 0x8000_0000 {
        println!("CPU model: brand string unavilable");
        return;
    }

    let mut brand_bytes = [0_u8; 48];
    for (index, leaf) in (0x8000_0002..=0x8000_0004).enumerate() {
        let result = unsafe { __cpuid(leaf) };
        let registers = [result.eax, result.ebx, result.ecx, result.edx];

        for (chunk, register) in brand_bytes[index * 16..(index + 1) * 16]
            .chunks_exact_mut(4)
            .zip(registers)
        {
            chunk.copy_from_slice(&register.to_le_bytes());
        }
    }

    let brand_len = brand_bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(brand_bytes.len());
    let brand = core::str::from_utf8(&brand_bytes[..brand_len])
        .unwrap_or("Unknown")
        .trim();

    println!("CPU model: {}", brand);
}

fn match_color(color_str: &str) -> Option<vga_buffer::Color> {
    match color_str {
        "black" => Some(vga_buffer::Color::Black),
        "blue" => Some(vga_buffer::Color::Blue),
        "green" => Some(vga_buffer::Color::Green),
        "cyan" => Some(vga_buffer::Color::Cyan),
        "red" => Some(vga_buffer::Color::Red),
        "magenta" => Some(vga_buffer::Color::Magenta),
        "brown" => Some(vga_buffer::Color::Brown),
        "lightgray" => Some(vga_buffer::Color::LightGray),
        "darkgray" => Some(vga_buffer::Color::DarkGray),
        "lightblue" => Some(vga_buffer::Color::LightBlue),
        "lightgreen" => Some(vga_buffer::Color::LightGreen),
        "lightcyan" => Some(vga_buffer::Color::LightCyan),
        "lightred" => Some(vga_buffer::Color::LightRed),
        "pink" => Some(vga_buffer::Color::Pink),
        "yellow" => Some(vga_buffer::Color::Yellow),
        "white" => Some(vga_buffer::Color::White),
        _ => None,
    }
}

fn sys_reboot() -> ! {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x64);
    unsafe {
        port.write(0xfe as u8);
    }
    loop {}
}

fn sys_shutdown() -> ! {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x604);
    unsafe {
        port.write(0x2000u16);
    }
    loop {
        x86_64::instructions::hlt();
    }
}

enum Command<'a> {
    Help,
    Uname,
    CpuInfo,
    MemInfo,
    Ticks,
    Cl,
    Version,
    About,
    Clear,
    Panic,
    Uptime,
    Time,
    History,
    Echo(&'a str),
    Color(&'a str),
    Vaasfetch, // ДОБАВИЛИ КАНДИДАТА В ENUM
    Shutdown,
    Reboot,
    Unknown,
}

static mut LAST_COMMAND: [u8; 80] = [0u8; 80];
static mut LAST_CMD_LEN: usize = 0;

// Регистрируем правильную точку входа через макрос загрузчика
bootloader::entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    println!("\n\n");
    println!("Welcome to vaas_os TTY Sh!");
    println!("Type 'help' for a list of available commands.");
    println!("--------------------------------------------");

    // ИСПРАВЛЕНО: Сначала шьем новую GDT и TSS, чтобы у процессора были валидные стеки!
    gdt::init();

    // Только теперь накатываем таблицу прерываний
    interrupts::init_idt();

    // Инициализируем контроллер прерываний
    unsafe { interrupts::PICS.lock().initialize() };

    // И только когда вся инфраструктура готова — врубаем прерывания
    x86_64::instructions::interrupts::enable();

    // === ИНИЦИАЛИЗАЦИЯ КУЧИ ===
    // УБИРАЕМ ХАРДКОД: вместо VirtAddr::new(0x_0000_4000_0000_0000);
    // БЕРЕМ РЕАЛЬНОЕ СМЕЩЕНИЕ, КОТОРОЕ НАМ ДАЛ БУТЛОАДЕР:
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

    // Настраиваем маппер страниц
    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // Передаем карту памяти в аллокатор фреймов
    let mut frame_allocator =
        unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_map) };

    // Запуск аллокатора страниц кучи
    allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("HEAP INITIALIZATION FAILED! Kernel panic.");

    println!("Heap allocator loaded successfully!");

    // ТЕСТ-ДРАЙВ ДИНАМИЧЕСКОЙ ПАМЯТИ
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    let test_box = Box::new(42);
    let mut test_vec = Vec::new();
    test_vec.push(1);
    test_vec.push(2);
    println!(
        "Heap test: Box value = {}, Vec capacity = {}",
        test_box,
        test_vec.capacity()
    );
    println!("--------------------------------------------");
    print!("vaash> ");

    // Наш главный цикл командной строки
    loop {
        if let Some(cmd_bytes) = interrupts::get_command() {
            let mut len = 0;
            while len < cmd_bytes.len() && cmd_bytes[len] != 0 {
                len += 1;
            }

            if let Ok(cmd_str) = core::str::from_utf8(&cmd_bytes[..len]) {
                let mut trimmed = cmd_str.trim();

                // Проверяем команду повтора истории
                unsafe {
                    if trimmed == "!!" {
                        if LAST_CMD_LEN > 0 {
                            if let Ok(last_str) =
                                core::str::from_utf8(&LAST_COMMAND[..LAST_CMD_LEN])
                            {
                                println!("{}", last_str);
                                trimmed = last_str;
                            }
                        } else {
                            println!("vaash: no commands in history yet.");
                            print!("vaash> ");
                            continue;
                        }
                    } else if !trimmed.is_empty() {
                        let bytes = trimmed.as_bytes();
                        let save_len = core::cmp::min(bytes.len(), 80);
                        LAST_COMMAND[..save_len].copy_from_slice(&bytes[..save_len]);
                        LAST_CMD_LEN = save_len;
                    }
                }

                let command = if trimmed == "help" {
                    Command::Help
                } else if trimmed == "uname" {
                    Command::Uname
                } else if trimmed == "cpuinfo" {
                    Command::CpuInfo
                } else if trimmed == "meminfo" {
                    Command::MemInfo
                } else if trimmed == "ticks" {
                    Command::Ticks
                } else if trimmed == "cl" {
                    Command::Cl
                } else if trimmed == "version" {
                    Command::Version
                } else if trimmed == "about" {
                    Command::About
                } else if trimmed == "clear" {
                    Command::Clear
                } else if trimmed == "panic" {
                    Command::Panic
                } else if trimmed == "uptime" {
                    Command::Uptime
                } else if trimmed == "time" {
                    Command::Time
                } else if trimmed == "history" {
                    Command::History
                } else if trimmed == "vaasfetch" {
                    Command::Vaasfetch
                } else if trimmed.starts_with("echo ") {
                    Command::Echo(&trimmed[5..])
                } else if trimmed == "echo" {
                    Command::Echo("")
                } else if trimmed.starts_with("color ") {
                    Command::Color(&trimmed[6..])
                } else if trimmed == "shutdown" {
                    Command::Shutdown
                } else if trimmed == "reboot" {
                    Command::Reboot
                } else {
                    Command::Unknown
                };

                match command {
                    Command::Help => {
                        println!("Available commands:");
                        println!(" help      - Show this manual");
                        println!(" uname     - Print system information");
                        println!(" cpuinfo   - Display CPU brand and details");
                        println!(" meminfo   - Display memory status");
                        println!(" ticks     - Show raw timer ticks (alias for time)");
                        println!(" cl        - Clear the screen (alias for clear)");
                        println!(" version   - Display kernel version info");
                        println!(" about     - More info about vaas_os");
                        println!(" clear     - Clear the screen");
                        println!(" echo      - Print text (e.g. echo hello)");
                        println!(" color     - Change text and background color (e.g. color red)");
                        println!(" uptime    - Show system uptime");
                        println!(" time      - Show raw timer ticks");
                        println!(" history   - Show last saved command");
                        println!(" vaasfetch - Display system info art");
                        println!(" reboot    - Restart the system");
                        println!(" shutdown  - Power off the machine");
                        println!(" panic     - Force trigger a kernel panic");
                    }
                    Command::Version => {
                        println!("vaash v0.1.0 (x86_64 bare-metal), built in 2026.")
                    }
                    Command::Clear | Command::Cl => crate::vga_buffer::clear(),
                    Command::Panic => panic!("User requested kernel panic!"),
                    Command::Echo(args) => println!("{}", args),
                    Command::Color(color_arg) => {
                        let mut parts = color_arg.split_whitespace();
                        let fg_str = parts.next().unwrap_or("");
                        let bg_str = parts.next().unwrap_or("black");

                        let fg_color = match_color(fg_str);
                        let bg_color = match_color(bg_str);

                        match (fg_color, bg_color) {
                            (Some(fg), Some(bg)) => {
                                crate::vga_buffer::set_color(fg, bg);
                            }
                            _ => {
                                println!("Usage: color <foreground> [background]");
                                println!("Unknown color combination: '{}' '{}'", fg_str, bg_str);
                            }
                        }
                    }
                    Command::About => {
                        println!("vaas_OS v0.1.0");
                        println!("Written by: Nikita");
                        println!("Language: Rust");
                        println!("Architecture: x86_64");
                        println!("Current status: Somehow works");
                    }
                    Command::Uptime => {
                        let seconds = interrupts::get_uptime_seconds();
                        println!("Uptime: {} seconds", seconds);
                    }
                    Command::Time | Command::Ticks => {
                        let ticks = unsafe { interrupts::TIMER_TICKS };
                        println!("Raw PIT Ticks: {}", ticks);
                    }
                    Command::History => unsafe {
                        if LAST_CMD_LEN > 0 {
                            if let Ok(last_str) =
                                core::str::from_utf8(&LAST_COMMAND[..LAST_CMD_LEN])
                            {
                                println!("Last command: '{}'", last_str);
                            }
                        } else {
                            println!("History is empty.");
                        }
                    },
                    Command::Uname => println!("vaas_os x86_64 bare-metal kernel v0.1.0-alpha"),
                    Command::MemInfo => {
                        // ИСПРАВЛЕНО: Теперь куча загружена, выводим реальный статус!
                        println!("MemTotal:     {} KiB", allocator::HEAP_SIZE / 1024);
                        println!("Heap Status:  Initialized & Running");
                    }
                    Command::Vaasfetch => {
                        let uptime_sec = interrupts::get_uptime_seconds();
                        let ticks = unsafe { interrupts::TIMER_TICKS };

                        // Логотип vaash
                        crate::vga_buffer::set_color(
                            vga_buffer::Color::LightGreen,
                            vga_buffer::Color::Black,
                        );
                        println!("  __      __                 _     ");
                        println!("  \\ \\    / /                | |    ");
                        println!("   \\ \\  / /_ _  __ _ ___  __| |__  ");
                        println!("    \\ \\/ / _` |/ _` / __|/ _` '_ \\ ");
                        println!("     \\  / (_| | (_| \\__ \\ (_| | | |");
                        println!("      \\/ \\__,_|\\__,_|___/\\__,_|_| |_|");

                        crate::vga_buffer::set_color(
                            vga_buffer::Color::LightGreen,
                            vga_buffer::Color::Black,
                        );
                        println!("-----------------------------------------");
                        println!("OS:        vaas_os v0.1.0 (x86_64 bare-metal)");
                        println!("Shell:     vaash v0.1.0");
                        println!("Uptime:    {} seconds (Ticks: {})", uptime_sec, ticks);
                        println!("Heap Size: {} KiB", allocator::HEAP_SIZE / 1024);
                        println!("Author:    Nikita");
                        println!("Status:    As stable as a bare-metal kernel can be");
                        println!("-----------------------------------------");
                    }
                    Command::Shutdown => {
                        println!("Shutting down...");
                        sys_shutdown();
                    }
                    Command::Reboot => {
                        println!("Restarting system...");
                        sys_reboot();
                    }
                    Command::Unknown => {
                        if !trimmed.is_empty() {
                            println!("Unknown command: '{}'", trimmed);
                        }
                    }
                    Command::CpuInfo => print_cpu_info(),
                }
                print!("vaash> ");
            }
        }
        x86_64::instructions::hlt();
    }
}
