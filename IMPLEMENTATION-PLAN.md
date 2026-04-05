# План реализации оптимизаций BK-Wiver

## 📋 Обзор

- **Проект:** BK-Wiver - система удаленного доступа
- **Цель:** Достижение 30 FPS и высокого качества изображения
- **Статус:** В разработке
- **Дата:** 2026-04-05

## 🎯 Целевые показатели

### Текущие показатели
- FPS: ~20-25
- Качество: HD/SD
- CPU usage: 60-80%

### Целевые показатели
- FPS: 30-40
- Качество: HD/4K
- CPU usage: 30-50%

## 📝 Файлы для изменения

### 1. Host/app/src/capture.rs

#### Изменения в захвате экрана
```rust
// Добавить адаптивное разрешение и буферизацию
pub struct CaptureFrame {
    pub image: RgbaImage,
    pub backend: &'static str,
    pub used_fallback: bool,
    pub capture_time: Instant,  // Добавить время захвата
    pub frame_index: u32,
}

pub struct CaptureEngine {
    #[cfg(windows)]
    inner: WindowsCaptureEngine,
    #[cfg(not(windows))]
    inner: ScreenshotsCaptureBackend,
    pub frame_buffer: Vec<RgbaImage>,  // Добавить буфер кадров
    pub frame_buffer_size: usize = 5,  // Размер буфера
}

impl CaptureEngine {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            inner: WindowsCaptureEngine::new(),
            #[cfg(not(windows))]
            inner: ScreenshotsCaptureBackend::with_backend_name("screenshots"),
            frame_buffer: Vec::new(),
            frame_buffer_size: 5,
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        // Оптимизация: добавляем кадры в буфер
        let frame_buffer_size = 5;
        let mut frame_buffer: Vec<RgbaImage> = Vec::new();
        let frame = RgbaImage::from_raw(max_dimensions.0, max_dimensions.1, ...);
        
        // Добавляем кадр в буфер
        frame_buffer.push(frame);
        
        // Увеличиваем буфер
        if frame_buffer.len() > frame_buffer_size {
            frame_buffer.remove(0);
        }
        
        CaptureFrame {
            image: frame,
            backend: "capture",
            used_fallback: false,
            capture_time: Instant::now(),
            frame_index,
        }
    }
}
```

#### Добавление адаптивного разрешения
```rust
fn get_optimal_resolution(network_quality: u8) -> (u32, u32) {
    match network_quality {
        90..=100 => (1920, 1080),  // Отлично
        70..=89  => (1280, 720),   // Хорошо
        50..=69  => (854, 480),    // Удовлетворительно
        _        => (640, 480),    // Плохо
    }
}

fn capture_frame(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
    // Адаптивное разрешение
    let dimensions = get_optimal_resolution(self.network_quality);
    
    CaptureFrame {
        image: self.inner.capture(dimensions, frame_index),
        backend: self.backend,
        used_fallback: self.inner.used_fallback(),
        capture_time: Instant::now(),
        frame_index,
    }
}
```

### 2. Host/app/src/media.rs

#### Изменения в кодеках
```rust
// Оптимальные настройки для кодеков
const H264_CRF: u8 = 25;        // Качество (20-30)
const H264_BITRATE: u64 = 5000000;  // 5 Mbps для 1080p
const H264_PRESSET: &str = "slow";      // Preset для качества
const VP8_BITRATE: u64 = 3000000;   // 3 Mbps для VP8
const VP8_FPS: u32 = 30;             // Целевой FPS
const VP8_KEYFRAME_INTERVAL: u32 = 50;

pub fn get_h264_config(bitrate: u64) -> H264Config {
    H264Config {
        crf: H264_CRF,
        bitrate: bitrate,
        preset: "slow",
        profile: "high",
        level: "4.1",
        keyframe_interval: 50,
    }
}

pub fn get_vp8_config(bitrate: u64) -> VP8Config {
    VP8Config {
        bitrate: bitrate,
        target_fps: VP8_FPS,
        keyframe_interval: VP8_KEYFRAME_INTERVAL,
    }
}
```

#### Оптимизация буферизации
```rust
pub fn predict_next_frame(&self, frames: &[Frame]) -> Frame {
    // Интерполяция между предыдущими кадрами
    if frames.len() >= 2 {
        let prev_frame = &frames[frames.len() - 2];
        let current_frame = &frames[frames.len() - 1];
        
        // Простая интерполяция
        let interpolated = interpolate_frames(&[prev_frame, current_frame]);
        interpolated
    } else {
        frames[frames.len() - 1].clone()
    }
}

fn interpolate_frames(frames: &[Frame]) -> Frame {
    // Интерполяция между кадрами
    let (prev_frame, current_frame) = (frames[0], frames[1]);
    
    // Простая интерполяция (можно усложнить)
    Frame {
        image: interpolate_image(&prev_frame.image, &current_frame.image),
        timestamp: Instant::now(),
        frame_index: frames[1].frame_index,
    }
}
```

### 3. Host/app/src/app.rs

#### Добавление мониторинга FPS
```rust
pub struct App {
    pub fps: u32,
    pub frame_timestamps: Vec<Instant>,
    pub network_quality: u8,
}

impl App {
    pub fn new() -> Self {
        Self {
            fps: 0,
            frame_timestamps: Vec::new(),
            network_quality: 100,
        }
    }

    pub fn update_fps(&mut self, capture_time: Instant) {
        // Добавляем кадр в буфер
        self.frame_timestamps.push(capture_time);
        
        // Ограничиваем буфер
        if self.frame_timestamps.len() > 30 {
            self.frame_timestamps.remove(0);
        }
        
        // Вычисляем FPS
        if self.frame_timestamps.len() >= 2 {
            let elapsed = self.frame_timestamps[1].duration_since(self.frame_timestamps[0]);
            let fps = 1_000_000_000.0 / elapsed.elapsed().as_nanos() as f64;
            self.fps = fps as u32;
        }
    }

    pub fn get_optimal_resolution(&self) -> (u32, u32) {
        // Адаптивное разрешение
        match self.network_quality {
            90..=100 => (1920, 1080),
            70..=89  => (1280, 720),
            50..=69  => (854, 480),
            _        => (640, 480),
        }
    }
}
```

#### Оптимизация захвата
```rust
impl App {
    pub fn capture_frame(&mut self) -> CaptureFrame {
        let max_dimensions = self.get_optimal_resolution();
        let capture_frame = self.capture_engine.capture(max_dimensions, self.frame_index);
        self.update_fps(capture_frame.capture_time);
        capture_frame
    }
}
```

## 🚀 Сборка и тестирование

### Шаги сборки
```bash
# 1. Установка Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# 2. Сборка проекта
cd /root/.openclaw/workspace/bk-wiver-project
cargo build --release -p bk-wiver-host

# 3. Тестирование
cargo test -p bk-wiver-host

# 4. Проверка качества
cargo run --release -p bk-wiver-host
```

### Команды проверки
```bash
# Проверка сборки
cargo check -p bk-wiver-host

# Проверка зависимостей
cargo tree -i <package>

# Профилирование
cargo profil -p bk-wiver-host --release
```

## 📊 Метрики производительности

### Измерение FPS
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

### Измерение качества
```rust
fn evaluate_quality(frame: &Frame) -> ImageQuality {
    ImageQuality {
        clarity: measure_clarity(&frame.image),
        color_accuracy: measure_color_accuracy(&frame.image),
        detail: measure_detail(&frame.image),
        ...
    }
}
```

## 🧪 Тестирование

### Сценарии тестирования
1. **Базовое тестирование**
   - Запуск приложения
   - Проверка захвата экрана
   - Проверка кодирования

2. **Тестирование производительности**
   - Измерение FPS
   - Мониторинг CPU/GPU
   - Проверка памяти

3. **Тестирование качества**
   - Оценка четкости
   - Проверка цветов
   - Анализ детализации

4. **Тестирование сети**
   - Низкая сеть (3G)
   - Средняя сеть (4G)
   - Высокая сеть (WiFi)

### Отчет о тестировании
```rust
struct PerformanceReport {
    fps: f64,
    quality_score: f64,
    cpu_usage: u8,
    gpu_usage: u8,
    memory_usage: u64,
    ...
}
```

## ✅ Контрольный список

### Задачи
- [x] Создание плана оптимизаций
- [x] Создание файлов документации
- [x] Создание скриптов тестирования
- [ ] Изменение capture.rs
- [ ] Изменение media.rs
- [ ] Изменение app.rs
- [ ] Тестирование оптимизаций
- [ ] Мониторинг производительности
- [ ] Добавление отчётности

### Файлы для создания
- [x] `OPTIMIZATION-IMPROVEMENTS.md` - рекомендации по оптимизации
- [x] `IMPLEMENTATION-PLAN.md` - план реализации
- [x] `test-optimizations.sh` - скрипт тестирования
- [ ] `performance-metrics.md` - отчёт о производительности

## 🎯 Следующие шаги

1. **Реализация изменений в коде**
   - Изменить capture.rs
   - Изменить media.rs
   - Изменить app.rs

2. **Сборка проекта**
   - Установка Rust
   - Сборка через cargo build
   - Тестирование сборки

3. **Тестирование оптимизаций**
   - Запуск проекта
   - Мониторинг FPS
   - Оценка качества

4. **Оптимизация параметров**
   - Адаптация кодеков
   - Настройка буферизации
   - Тонкая настройка

5. **Отчётность**
   - Запись метрик
   - Анализ производительности
   - Улучшение результатов

---

**Дата создания:** 2026-04-05  
**Версия проекта:** v0.1.12  
**Статус:** В реализации
