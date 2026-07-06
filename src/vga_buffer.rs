use core::fmt;
use spin::Mutex;

/// Перечисление стандартных цветов VGA
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// Полноценный байт цвета (цвет текста + цвет фона)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ColorCode(pub u8);

impl ColorCode {
    pub fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

/// Структура, описывающая один символ на экране
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

// Стандартные размеры текстового экрана VGA
const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

/// Представление самого буфера памяти VGA
#[repr(transparent)]
struct Buffer {
    chars: [[ScreenChar; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// Главная структура для управления выводом текста
pub struct Writer {
    row_position: usize,
    column_position: usize,
    color_code: ColorCode,
    buffer: *mut Buffer,
}

// Явно говорим компилятору, что Writer можно пересылать между потоками
unsafe impl Send for Writer {}

// Явно говорим, что к Writer можно безопасно обращаться по разделяемой ссылке (так как он под Мутексом)
unsafe impl Sync for Writer {}

impl Writer {
    /// Пишет один ASCII-байт на экран
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            b'\t' => {
            // Вычисляем, сколько пробелов нужно до следующей метки табуляции
            let spaces = 4 - (self.column_position % 4);
            
            for _ in 0..spaces {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;
                let color_code = self.color_code;

                // ОБЯЗАТЕЛЬНО пишем пробел в память, чтобы стереть то, что там было раньше
                unsafe {
                    (*self.buffer).chars[row][col] = ScreenChar {
                        ascii_character: b' ', // Пишем пустой символ
                        color_code,
                    };
                }
                
                self.column_position += 1;
            }
        }
            byte => {
            if self.column_position >= BUFFER_WIDTH {
                self.new_line();
            }

            let row = BUFFER_HEIGHT - 1;
            let col = self.column_position;

            let color_code = self.color_code;
            
            unsafe {
                // Просто присваиваем значение ячейке памяти напрямую
                (*self.buffer).chars[row][col] = ScreenChar {
                    ascii_character: byte,
                    color_code,
                };
            }
            self.column_position += 1;
        }
        }
    }

    /// Перенос строки и скроллинг экрана вверх
    fn new_line(&mut self) {
    // 1. Сдвигаем все строки со 2-й по последнюю на одну позицию вверх
    for row in 1..BUFFER_HEIGHT {
        for col in 0..BUFFER_WIDTH {
            unsafe {
                // Копируем символ из нижней строки в верхнюю
                let character = (*self.buffer).chars[row][col];
                (*self.buffer).chars[row - 1][col] = character;
            }
        }
    }

    // 2. Очищаем самую нижнюю строку, чтобы на ней можно было писать заново
    self.clear_row(BUFFER_HEIGHT - 1);
    
    // Возвращаем курсор в начало нижней строки
    self.column_position = 0;
}

// Вспомогательный метод для очистки конкретной строки пробелами
fn clear_row(&mut self, row: usize) {
    let blank = ScreenChar {
        ascii_character: b' ',
        color_code: self.color_code,
    };
    for col in 0..BUFFER_WIDTH {
        unsafe {
            (*self.buffer).chars[row][col] = blank;
        }
    }
}

    /// Вывод целой строки
    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
        }
    }

    /// Удаление последнего символа (обработка Backspace)
    pub fn delete_last_char(&mut self) {
        // Так как мы всегда пишем на самой нижней строке:
        let row = BUFFER_HEIGHT - 1;

        if self.column_position > 0 {
            // Если мы не в самом начале строки, просто двигаемся влево
            self.column_position -= 1;
            let col = self.column_position;
            let color_code = self.color_code;

            unsafe {
                // Используем write_volatile для bare-metal памяти VGA
                core::ptr::write_volatile(
                    &mut (*self.buffer).chars[row][col],
                    ScreenChar {
                        ascii_character: b' ', // Затираем пробелом
                        color_code,
                    },
                );
            }
        } else {
            // Идея на будущее: если column_position == 0, то в полноценном Linux TTY
            // курсор прыгает на конец ПРЕДЫДУЩЕЙ строки.
            // Но так как у тебя включен постоянный скроллинг вверх и данные из прошлых строк
            // улетели, прыгать на строку выше (row - 1) небезопасно, там текст уже "запечен".
            // Поэтому здесь на самом начале строки (vaas_os> ) мы просто ничего не делаем,
            // чтобы не стереть сам промпт шелла.
        }
    }

    pub fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        // row_position нам больше не нужен, но column_position сбрасываем в 0
        self.column_position = 0;
    }

    pub fn set_color(&mut self, foreground: Color, background: Color) {
        self.color_code = ColorCode::new(foreground, background);
    }
}

pub fn clear() {
    WRITER.lock().clear_screen();
}

pub fn backspace() {
    WRITER.lock().delete_last_char();
}

pub fn set_color(foreground: Color, background: Color) {
    WRITER.lock().set_color(foreground, background);
}

/// Реализуем трейт форматирования для макроса write!
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

/// НАШ СТАТИЧЕСКИЙ ГЛОБАЛЬНЫЙ ИНТЕРФЕЙС (СИНГЛТОН)
pub static WRITER: Mutex<Writer> = Mutex::new(Writer {
    row_position: 0,
    column_position: 0,
    color_code: ColorCode((Color::Black as u8) << 4 | (Color::LightGreen as u8)),
    buffer: 0xb8000 as *mut Buffer,
});

/// Функция, которую будут дергать макросы print! и println! под капотом
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    WRITER.lock().write_fmt(args).unwrap();
}

/// Макрос print!
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

/// Макрос println!
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
