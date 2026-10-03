use core::panic;

use crate::gdt;
use crate::print;
use crate::println;
use pic8259::{self, ChainedPics};
use spin::Lazy;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

// Базовое смещение для прерываний аппаратной части.
// Векторы 0..31 заняты процессором под исключения. Мапим PIC на 32..47.
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

// Номера векторов прерываний для наших девайсов
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET, // 32
    Keyboard,             // 33
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
    fn as_usize(self) -> usize {
        self as usize
    }
}

static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

    // Исключения процессора
    idt.breakpoint.set_handler_fn(breakpoint_handler);

    // ДОБАВЛЯЕМ ОБРАБОТЧИКИ ДЛЯ ОПРЕДЕЛЕНИЯ КОРНЯ ПРОБЛЕМЫ:
    idt.general_protection_fault
        .set_handler_fn(general_protection_fault_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);

    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }

    // Аппаратные прерывания
    idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_handler);
    idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_handler);

    idt
});

use x86_64::structures::idt::PageFaultErrorCode;

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!(
        "EXCEPTION: GENERAL PROTECTION FAULT\nError Code: {:#x}\n{:#?}",
        error_code, stack_frame
    );
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    panic!(
        "EXCEPTION: PAGE FAULT\nAccessed Address: {:?}\nError Code: {:?}\n{:#?}",
        Cr2::read(), // Регистр CR2 содержит виртуальный адрес, из-за которого всё упало
        error_code,
        stack_frame
    );
}

pub fn init_idt() {
    IDT.load();
}

// --- Обработчики исключений процессора ---

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

// --- Обработчики внешних прерываний оборудования ---

pub static mut TIMER_TICKS: u64 = 0;

// Прерывание системного таймера (тикает постоянно)
extern "x86-interrupt" fn timer_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        TIMER_TICKS += 1;
    }
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

// --- БУФЕР ДЛЯ НАШЕГО TTY ---
const CMD_BUFFER_SIZE: usize = 80;
// Явно пишем 0u8, чтобы Rust сразу понял, что это массив байт
static mut CMD_BUFFER: [u8; CMD_BUFFER_SIZE] = [0u8; CMD_BUFFER_SIZE];
static mut CMD_LEN: usize = 0;
static mut CMD_READY: bool = false;

static mut SHIFT_PRESSED: bool = false;

/// Функция для извлечения готовой команды (исправили опечатку в названии!)
pub fn get_command() -> Option<[u8; CMD_BUFFER_SIZE]> {
    x86_64::instructions::interrupts::without_interrupts(|| unsafe {
        if CMD_READY {
            let res = CMD_BUFFER;
            // И здесь заменяем на чистый инициализатор без 'as'
            CMD_BUFFER = [0u8; CMD_BUFFER_SIZE];
            CMD_LEN = 0;
            CMD_READY = false;
            Some(res)
        } else {
            None
        }
    })
}

pub fn get_uptime_seconds() -> u64 {
    unsafe { TIMER_TICKS / 18 }
}

static mut EXTENDED_SCANCODE: bool = false;

// Прерывание клавиатуры (вызывается при нажатии и отпускании клавиш)
extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    unsafe {
        match scancode {
            0x4b => {
                // Стрелка влево нажата
            }
            0x4d => {
                // Стрелка вправо нажата
            }
            // Нажатие Левого или Правого Shift
            0x2a | 0x36 => {
                SHIFT_PRESSED = true;
            }
            // Отпускание Левого или Правого Shift (скан-код + 0x80)
            0xaa | 0xb6 => {
                SHIFT_PRESSED = false;
            }
            // Обрабатываем остальные нажатия клавиш (коды < 0x80)
            0x00..=0x7f => {
                if !CMD_READY {
                    match scancode {
                        // Нажатие Enter
                        0x1c => {
                            print!("\n");
                            CMD_READY = true;
                        }
                        // Нажатие Backspace
                        0x0e => {
                            if CMD_LEN > 0 {
                                CMD_LEN -= 1;
                                CMD_BUFFER[CMD_LEN] = 0;
                                crate::vga_buffer::backspace();
                            }
                        }
                        // Все остальные символы
                        _ => {
                            let key = match scancode {
                                // Цифровая строка и символы
                                0x02 => Some(if SHIFT_PRESSED { b'!' } else { b'1' }),
                                0x03 => Some(if SHIFT_PRESSED { b'@' } else { b'2' }),
                                0x04 => Some(if SHIFT_PRESSED { b'#' } else { b'3' }),
                                0x05 => Some(if SHIFT_PRESSED { b'$' } else { b'4' }),
                                0x06 => Some(if SHIFT_PRESSED { b'%' } else { b'5' }),
                                0x07 => Some(if SHIFT_PRESSED { b'^' } else { b'6' }),
                                0x08 => Some(if SHIFT_PRESSED { b'&' } else { b'7' }),
                                0x09 => Some(if SHIFT_PRESSED { b'*' } else { b'8' }),
                                0x0a => Some(if SHIFT_PRESSED { b'(' } else { b'9' }),
                                0x0b => Some(if SHIFT_PRESSED { b')' } else { b'0' }),
                                0x0c => Some(if SHIFT_PRESSED { b'_' } else { b'-' }),
                                0x0d => Some(if SHIFT_PRESSED { b'+' } else { b'=' }),

                                // Буквы (если Shift нажат — переводим в заглавные)
                                0x10 => Some(if SHIFT_PRESSED { b'Q' } else { b'q' }),
                                0x11 => Some(if SHIFT_PRESSED { b'W' } else { b'w' }),
                                0x12 => Some(if SHIFT_PRESSED { b'E' } else { b'e' }),
                                0x13 => Some(if SHIFT_PRESSED { b'R' } else { b'r' }),
                                0x14 => Some(if SHIFT_PRESSED { b'T' } else { b't' }),
                                0x15 => Some(if SHIFT_PRESSED { b'Y' } else { b'y' }),
                                0x16 => Some(if SHIFT_PRESSED { b'U' } else { b'u' }),
                                0x17 => Some(if SHIFT_PRESSED { b'I' } else { b'i' }),
                                0x18 => Some(if SHIFT_PRESSED { b'O' } else { b'o' }),
                                0x19 => Some(if SHIFT_PRESSED { b'P' } else { b'p' }),
                                0x1e => Some(if SHIFT_PRESSED { b'A' } else { b'a' }),
                                0x1f => Some(if SHIFT_PRESSED { b'S' } else { b's' }),
                                0x20 => Some(if SHIFT_PRESSED { b'D' } else { b'd' }),
                                0x21 => Some(if SHIFT_PRESSED { b'F' } else { b'f' }),
                                0x22 => Some(if SHIFT_PRESSED { b'G' } else { b'g' }),
                                0x23 => Some(if SHIFT_PRESSED { b'H' } else { b'h' }),
                                0x24 => Some(if SHIFT_PRESSED { b'J' } else { b'j' }),
                                0x25 => Some(if SHIFT_PRESSED { b'K' } else { b'k' }),
                                0x26 => Some(if SHIFT_PRESSED { b'L' } else { b'l' }),
                                0x2c => Some(if SHIFT_PRESSED { b'Z' } else { b'z' }),
                                0x2d => Some(if SHIFT_PRESSED { b'X' } else { b'x' }),
                                0x2e => Some(if SHIFT_PRESSED { b'C' } else { b'c' }),
                                0x2f => Some(if SHIFT_PRESSED { b'V' } else { b'v' }),
                                0x30 => Some(if SHIFT_PRESSED { b'B' } else { b'b' }),
                                0x31 => Some(if SHIFT_PRESSED { b'N' } else { b'n' }),
                                0x32 => Some(if SHIFT_PRESSED { b'M' } else { b'm' }),

                                // Пробел
                                0x39 => Some(b' '),
                                _ => None,
                            };

                            if let Some(byte) = key {
                                if CMD_LEN < CMD_BUFFER_SIZE - 1 {
                                    CMD_BUFFER[CMD_LEN] = byte;
                                    CMD_LEN += 1;
                                    print!("{}", byte as char);
                                }
                            }
                        }
                    }
                }
            }
            // Все остальные брейк-коды (отпускания клавиш) просто игнорируем
            _ => {}
        }
    }

    // Сообщаем PIC, что обработка завершена
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}
