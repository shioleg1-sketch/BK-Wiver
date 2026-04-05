#!/bin/bash
# Быстрая проверка состояния проекта BK-Wiver
# Этот скрипт работает без Rust

echo "=== Быстрая проверка BK-Wiver v0.1.30 ==="
echo ""

echo "1. Проверка Git..."
git --version 2>/dev/null && echo "   ✓ Git установлен: $(git --version)" || echo "   ✗ Git не установлен"

echo ""
echo "2. Проверка Git репозитория..."
if [ -d ".git" ]; then
    echo "   ✓ Репозиторий инициализирован"
    echo "   Последняя ветка: $(git branch --show-current)"
    echo "   Локальные коммиты: $(git rev-list --count HEAD)"
else
    echo "   ✗ Репозиторий не инициализирован"
fi

echo ""
echo "3. Проверка документации..."
docs=0
for doc in README.md INSTALLATION-INSTRUCTIONS.md USAGE-INSTRUCTIONS.md; do
    if [ -f "$doc" ]; then
        echo "   ✓ $doc"
        docs=$((docs + 1))
    else
        echo "   ✗ $doc не найдено"
    fi
done
echo "   Документация: $docs файлов"

echo ""
echo "4. Проверка зависимостей в Cargo.toml..."
if [ -f "Host/app/Cargo.toml" ]; then
    echo "   ✓ Host/app/Cargo.toml существует"
    echo "   Версия проекта: $(grep '^version =' Host/app/Cargo.toml | cut -d'=' -f2 | tr -d '"' | tr -d ' ')"
    echo "   Edition: $(grep '^edition =' Host/app/Cargo.toml | cut -d'=' -f2 | tr -d '"' | tr -d ' ')"
else
    echo "   ✗ Host/app/Cargo.toml не найден"
fi

echo ""
echo "5. Проверка документации проекта..."
for doc in OPTIMIZATION-SUMMARY.md PROJECT-COMPLETE.md PROJECT-STATUS.md; do
    if [ -f "$doc" ]; then
        echo "   ✓ $doc"
    fi
done

echo ""
echo "6. Проверка скриптов..."
if [ -f "test-optimizations.sh" ]; then
    echo "   ✓ test-optimizations.sh"
    chmod +x test-optimizations.sh 2>/dev/null
else
    echo "   ✗ test-optimizations.sh не найден"
fi

echo ""
echo "=== Результаты проверки ==="
echo "Документация: $docs файлов"
if [ -d ".git" ]; then
    echo "Гит репозиторий: активен"
fi
if [ -f "Host/app/Cargo.toml" ]; then
    echo "Cargo.toml: существует"
fi

echo ""
echo "=== Рекомендации ==="
echo "1. Установите Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
echo "2. Соберите проект: cargo build --release -p bk-wiver-host"
echo "3. Проверьте производительность: cargo bench -p bk-wiver-host"
echo ""
echo "=== Статус проверки: УСПЕШНО (без Rust) ==="
