# Установка и сборка BK-Wiver v0.1.30

## 📋 Требования

### Rust (необходим для сборки):

#### На Linux:
```bash
# Установка Rust через rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Проверка установки
rustc --version
cargo --version
```

#### На Windows (PowerShell):
```powershell
# Установка Rust через winget
winget install Rustlang.Rustup

# Или вручную
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | powershell -ExecutionPolicy Bypass -
```

### System Dependencies:

#### На Linux (Ubuntu/Debian):
```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev
```

#### На Windows:
Установите Visual Studio Build Tools:
1. Скачайте с https://visualstudio.microsoft.com/downloads/
2. Установите "Desktop development with C++"
3. Включите "C++ CMake tools" и "CMake"

## 🔨 Сборка проекта

### 1. Клонирование репозитория:
```bash
git clone https://github.com/shioleg1-sketch/BK-Wiver.git
cd BK-Wiver
```

### 2. Установка зависимостей:
```bash
# Установка Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Сборка проекта
cargo build --release -p bk-wiver-host

# Проверка сборки
cargo check -p bk-wiver-host
```

### 3. Компиляция в исполняемый файл:
```bash
# Автоматическая компиляция .exe
cargo build --release -p bk-wiver-host

# Исполняемый файл будет в:
# Linux: target/release/bk-wiver-host
# Windows: target/release/bk-wiver-host.exe
```

### 4. Проверка сборки:
```bash
# Проверка кода
cargo clippy -p bk-wiver-host

# Тестирование производительности
cargo bench -p bk-wiver-host

# Проверка зависимостей
cargo tree -d
```

## 🚀 Запуск проекта

### На Linux:
```bash
# Запуск проекта
./target/release/bk-wiver-host

# Запуск с параметрами
./target/release/bk-wiver-host --help

# Запуск в фоне
./target/release/bk-wiver-host &
```

### На Windows:
```powershell
# Запуск проекта
.\target\release\bk-wiver-host.exe

# Запуск с правами администратора
.\target\release\bk-wiver-host.exe -Administrator
```

## 📊 Настройка проекта

### 1. Конфигурация качества изображения:

Откройте `Host/app/src/` и найдите файл настройки качества.

#### Оптимальные настройки:
- **Bitrate:** 4-6 Mbps
- **CRF:** 25
- **Keyframe:** каждые 50 кадров
- **Разрешение:** 
  - WiFi: 1920x1080
  - 4G: 1280x720
  - 3G: 640x480

### 2. Настройка кодеков:

#### NVENC (NVIDIA):
```toml
# Host/app/Cargo.toml
[target.'cfg(windows)'.dependencies]
nvenc = "0.1"
```

#### QSV (Intel):
```toml
[target.'cfg(windows)'.dependencies]
intel-media = "0.8"
```

#### AMF (AMD):
```toml
[target.'cfg(windows)'.dependencies]
amf = "0.5"
```

## 🐛 Отладка и диагностирование

### 1. Включение отладочной информации:
```bash
RUST_LOG=debug cargo run --release -p bk-wiver-host
```

### 2. Проверка памяти:
```bash
cargo valgrind -p bk-wiver-host  # Linux только
```

### 3. Проверка памяти (Windows):
```powershell
# Использование Windows Performance Analyzer
# Запустите проект под WPA для анализа производительности
```

### 4. Мониторинг производительности:
```bash
# Linux (htop, mpstat)
htop
mpstat -P ALL 5

# Windows (Resource Monitor)
resourceMonitor.exe
```

## 📝 Обновление проекта

### 1. Обновление зависимостей:
```bash
cargo update -p bk-wiver-host
cargo update
```

### 2. Обновление кода:
```bash
git pull origin main
cargo build --release -p bk-wiver-host
```

### 3. Решение проблем со сборкой:

#### Ошибка зависимостей:
```bash
# Очистка и повторная установка
cargo clean
cargo build --release -p bk-wiver-host
```

#### Ошибка компиляции:
```bash
# Проверка совместимости версий
rustc --version
cargo --version

# Обновление Rust
rustup update
```

## 🎯 Оптимизация производительности

### 1. Использование hardware acceleration:
- **NVENC:** для NVIDIA GPU
- **QSV:** для Intel GPU
- **AMF:** для AMD GPU
- **VideoToolbox:** для macOS

### 2. Настройка буферизации:
```toml
# Host/app/Cargo.toml
crossbeam-channel = "0.5"  # Буферизация кадров
```

### 3. Оптимизация WebP:
```toml
# Оптимизированный пакет
webp = "0.3.1"  # Быстрая обработка изображений
```

### 4. Параллельная обработка:
```toml
# Использование многопоточности
crossbeam-channel = "0.5"  # Мультипоточная буферизация
```

## 🔧 Настройка окружения

### Linux (.bashrc):
```bash
export RUST_BACKTRACE=1
export RUST_LOG=info
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=ld.lld
```

### Windows (System Environment Variables):
- `RUST_BACKTRACE`: 1
- `RUST_LOG`: info
- `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER`: lld-link.exe

## 📚 Дополнительные ресурсы

### Документация:
- **README.md**: Основы проекта
- **USAGE-INSTRUCTIONS.md**: Подробная инструкция по использованию
- **OPTIMIZATION-SUMMARY.md**: Итоговая документация по оптимизации
- **PROJECT-COMPLETE.md**: Подтверждение завершения оптимизации

### Скрипты:
- **test-optimizations.sh**: Тестирование оптимизаций
- **deploy-to-wiver.sh**: Деплой на сервер

### GitHub:
- **Репозиторий**: https://github.com/shioleg1-sketch/BK-Wiver
- **Latest commit**: 66d7fe7 (подтверждение завершения оптимизации)

## ✅ Проверка правильной установки

```bash
# Проверка всех компонентов
echo "Rust version:"
rustc --version

echo "Cargo version:"
cargo --version

echo "Project location:"
pwd

echo "Executable location:"
ls -lh target/release/bk-wiver-host*

echo "Build successful!"
```

## 💡 Подсказки

- Используйте `cargo check` для быстрой проверки кода
- Используйте `cargo clippy` для улучшения качества кода
- Используйте `cargo bench` для тестирования производительности
- Используйте `cargo doc` для генерации документации

## 🎓 Обучение

### 1. Изучите структуру проекта:
- `Host/app/src/` - Исходный код приложения
- `Host/app/Cargo.toml` - Зависимости проекта
- `Host/app/Cargo.lock` - Зафиксированные версии зависимостей

### 2. Изучите код:
```bash
# Просмотр исходного кода
tree Host/app/src/

# Поиск ключевых функций
grep -r "capture" Host/app/src/
```

### 3. Изучите оптимизации:
- `OPTIMIZATION-SUMMARY.md` - Итоговая документация
- `USAGE-INSTRUCTIONS.md` - Использование оптимизаций
- `PROJECT-COMPLETE.md` - Подтверждение завершения

---

**Версия проекта:** v0.1.30  
**Статус:** Оптимизация завершена  
**Дата:** 2026-04-05  
**Репозиторий:** https://github.com/shioleg1-sketch/BK-Wiver
