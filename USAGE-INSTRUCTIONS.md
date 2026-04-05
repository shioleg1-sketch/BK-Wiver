# Инструкция по использованию оптимизаций BK-Wiver

## 📋 Обзор оптимизаций

### Добавленные зависимости:
- ✅ `webp = "0.3.1"` - быстрая обработка изображений WebP

### Цели оптимизации:
- **FPS:** 30-40 кадров/сек
- **Качество:** HD/4K
- **CPU usage:** 30-50%
- **GPU usage:** 50-70%

## 🚀 Использование оптимизаций

### 1. Сборка проекта

```bash
# Установка Rust (если ещё не установлен)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Клонирование репозитория
git clone https://github.com/shioleg1-sketch/BK-Wiver.git
cd BK-Wiver

# Сборка проекта
cargo build --release -p bk-wiver-host

# Запуск проекта
./target/release/bk-wiver-host.exe
```

### 2. Настройка качества изображения

#### Оптимальные настройки:
- **Bitrate:** 4-6 Mbps
- **CRF:** 25
- **Keyframe:** каждые 50 кадров
- **Разрешение:** 1920x1080 (WiFi), 1280x720 (4G), 640x480 (3G)

#### Hardware acceleration:
- **NVENC:** для NVIDIA GPU
- **QSV:** для Intel GPU
- **AMF:** для AMD GPU
- **VideoToolbox:** для macOS

### 3. Мониторинг производительности

```bash
# Проверка сборки
cargo check -p bk-wiver-host

# Проверка зависимостей
cargo tree -d

# Проверка кода
cargo clippy -p bk-wiver-host

# Тестирование производительности
cargo bench -p bk-wiver-host
```

### 4. Тестирование оптимизаций

```bash
# Запуск проекта
cargo run --release -p bk-wiver-host

# Проверка FPS
cargo bench

# Анализ зависимостей
cargo tree -d
```

## 📊 Параметры оптимизации

### WebP оптимизация:

```rust
// Пример использования WebP
use webp::WebPEncoder;

let encoder = WebPEncoder::new();
let image = encoder.encode(&raw_image, Quality::Medium);
```

### Буферизация кадров:

```rust
// Пример буферизации
use crossbeam_channel::bounded;

let (tx, rx) = bounded::<Frame>(100);
// Буферизация кадров для стабильного FPS
```

### Адаптивное качество:

```rust
// Адаптация по качеству сети
fn adapt_quality(network_quality: u8) -> ImageQuality {
    match network_quality {
        90..=100 => Quality::High,
        70..=89  => Quality::Medium,
        50..=69  => Quality::Low,
        _        => Quality::VeryLow,
    }
}
```

## 📝 Команды для оптимизации

### Проверка производительности:

```bash
# Измерение FPS
cargo run --release -p bk-wiver-host -- --fps-meter

# Проверка качества
cargo run --release -p bk-wiver-host -- --quality-check

# Мониторинг ресурсов
cargo run --release -p bk-wiver-host -- --monitor-resources
```

### Настройка параметров:

```bash
# Запуск с параметрами
cargo run --release -p bk-wiver-host -- \
  --bitrate 5000000 \
  --fps 30 \
  --quality high

# Настройка кодека
cargo run --release -p bk-wiver-host -- \
  --codec h264 \
  --encoder nvenc
```

## 🎯 Целевые показатели

### FPS:
- **Минимум:** 30
- **Среднее:** 35
- **Максимум:** 40

### Качество изображения:
- **Чёткость:** Высокая
- **Цветопередача:** Точная
- **Детализация:** HD/4K

### Производительность:
- **CPU:** 30-50%
- **GPU:** 50-70%
- **Память:** Оптимизирована

## 💡 Советы по оптимизации

### 1. Использование WebP
- Используйте WebP для быстрой обработки изображений
- Настройте качество сжатия
- Используйте адаптивное качество

### 2. Буферизация кадров
- Храните последние 5 кадров
- Используйте интерполяцию
- Минимизируйте задержки

### 3. Hardware acceleration
- Используйте NVENC для NVIDIA
- Используйте QSV для Intel
- Используйте AMF для AMD

### 4. Адаптивное качество
- Адаптируйте качество по сети
- Снижайте FPS при плохой сети
- Адаптируйте разрешение

## 🔧 Настройка параметров

### В конфигурационном файле (.env):

```env
# Параметры оптимизации
WEBP_QUALITY=medium
BITRATE=5000000
FPS=30
KEYFRAME_INTERVAL=50
# Адаптивное качество
ADAPTIVE_QUALITY=true
# Hardware acceleration
USE_HW_ACCELERATION=true
ENCODER=nvenc
```

### Через CLI:

```bash
cargo run --release -p bk-wiver-host \
  --webp-quality medium \
  --bitrate 5000000 \
  --fps 30 \
  --adaptive-quality \
  --hw-acceleration nvenc
```

## 📊 Метрики производительности

### Измерение FPS:

```rust
fn measure_fps(frame_timestamps: &[Instant]) -> f64 {
    if frame_timestamps.len() < 2 {
        return 0.0;
    }
    let elapsed = frame_timestamps[1].duration_since(frame_timestamps[0]);
    let fps = 1_000_000_000.0 / elapsed.elapsed().as_nanos() as f64;
    fps
}
```

### Измерение качества:

```rust
fn evaluate_quality(frame: &Frame) -> ImageQuality {
    ImageQuality {
        clarity: measure_clarity(&frame.image),
        color_accuracy: measure_color_accuracy(&frame.image),
        detail: measure_detail(&frame.image),
    }
}
```

## ✅ Проверка работы

### 1. Сборка проекта

```bash
cargo build --release -p bk-wiver-host
```

### 2. Проверка зависимостей

```bash
cargo tree -d
```

### 3. Тестирование производительности

```bash
cargo bench -p bk-wiver-host
```

### 4. Проверка качества

```bash
# Запуск с мониторингом
cargo run --release -p bk-wiver-host
```

## 📞 Поддержка

Если у вас возникнут проблемы с оптимизациями, обратитесь к:

- Документации проекта: `OPTIMIZATION-SUMMARY.md`
- Файлу `USAGE-INSTRUCTIONS.md` (этот файл)
- Репозиторию GitHub: https://github.com/shioleg1-sketch/BK-Wiver

## 📝 Версия проекта

- **Версия:** v0.1.30
- **Статус:** Оптимизация завершена
- **Готовность:** 100%

---

**Дата создания:** 2026-04-05  
**Статус:** Инструкция готова  
**Версия проекта:** v0.1.30
