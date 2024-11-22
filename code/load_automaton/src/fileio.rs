use anyhow::Result;
use fs_err as fs;
use num::Integer;

use std::error;
use std::fmt;
use std::io::prelude::*;

#[derive(Debug, Clone)]
pub struct ParseError;

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Ошибка перевода строки в число")
    }
}
impl error::Error for ParseError {}

#[derive(Debug, Clone)]
pub struct NonPositiveError;

impl std::fmt::Display for NonPositiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Число отрицательно")
    }
}

impl error::Error for NonPositiveError {}

pub fn scan_integer<T: Integer + std::str::FromStr>() -> Result<T> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    line = line.trim().to_string();
    let res = match line.parse::<T>() {
        Ok(number) => number,
        Err(_e) => {
            eprintln!("Ошибка сканирования числа");
            return Err(ParseError.into());
        }
    };
    Ok(res)
}

#[allow(dead_code)]
pub fn scan_nonnegative_number<T: Integer + std::str::FromStr>() -> Result<T> {
    let res = scan_integer()?;
    if res < T::zero() {
        return Err(NonPositiveError.into());
    }
    Ok(res)
}

#[allow(dead_code)]
pub fn scan_nonnegative_number_prompt<T: Integer + std::str::FromStr>(prompt: &str) -> Result<T> {
    print!("{}", prompt);
    std::io::stdout()
        .flush()
        .expect("Невозможно сбросить буфер потока вывода");
    scan_nonnegative_number::<T>()
}

#[allow(dead_code)]
pub fn read_line() -> String {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("Ошибка чтения строки из потока ввода");

    line
}

#[allow(dead_code)]
pub fn read_prompt(prompt: &str) -> String {
    print!("{}", prompt);
    std::io::stdout()
        .flush()
        .expect("Невозможно сбросить буфер потока вывода");
    let line = read_line();

    line.strip_suffix("\r\n")
        .or(line.strip_suffix("\n"))
        .unwrap_or(line.as_str())
        .to_owned()
}

#[allow(dead_code)]
pub fn create_file_from_string(name: &str, data: &String) -> Result<()> {
    let mut file_out = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&name)
        .expect("Не удалось создать файл");

    if let Err(e) = file_out.write_all(data.as_bytes()) {
        eprintln!("Ошибка при записи в файл: {}", e);
    };

    Ok(())
}

#[allow(dead_code)]
pub fn recreate_dir_all(path: &std::path::Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

#[allow(dead_code)]
pub fn input_vector() -> Result<Vec<i32>> {
    let mut line: String = read_line();
    line = line.trim().to_string();
    let parts = line.split(" ").collect::<Vec<&str>>();
    let elements_amount = parts.len();
    if elements_amount == 0 {
        let err = NonPositiveError;
        return Err(err.into());
    }

    let mut vector: Vec<i32> = Vec::new();

    for &part in parts.iter() {
        let number: i32 = match part.parse::<i32>() {
            Ok(num) => num,
            Err(e) => {
                println!("Ошибка сканирования числа");
                return Err(e.into());
            }
        };
        vector.push(number);
    }

    Ok(vector)
}
