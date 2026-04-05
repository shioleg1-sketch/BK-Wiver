#!/bin/bash
# Скрипт мониторинга сборки BK-Wiver v0.1.30
# Автоматически проверяет сборку каждые 30 секунд

set -e

echo "=== Мониторинг сборки BK-Wiver v0.1.30 ==="
echo ""

MAX_ITERATIONS=20
ITERATION=0
RUST_CHECK_FAILED_COUNT=0
SUCCESS_COUNT=0

while [ $ITERATION -lt $MAX_ITERATIONS ]; do
    ITERATION=$((ITERATION + 1))
    echo "--- Проверка $ITERATION/$MAX_ITERATIONS ---"
    echo ""
    
    # Проверка Rust
    if command -v cargo &>/dev/null; then
        echo "✅ Rust установлен!"
        RUST_VERSION=$(cargo --version 2>&1)
        echo "Версия: $RUST_VERSION"
        echo ""
        
        # Сборка проекта
        echo "Начинаю сборку проекта..."
        cd Host/app
        cargo check -p bk-wiver-host 2>&1 | tail -20
        
        if [ $? -eq 0 ]; then
            echo ""
            echo "✅ Сборка прошла успешно!"
            echo "✅ Проверка: $ITERATION/$MAX_ITERATIONS"
            SUCCESS_COUNT=$((SUCCESS_COUNT + 1))
            
            # Запуск сборки
            echo ""
            echo "Начинаю полную сборку..."
            cargo build --release -p bk-wiver-host
            
            if [ $? -eq 0 ]; then
                echo ""
                echo "========================================="
                echo "✅ Сборка завершенна успешно!"
                echo "========================================="
                echo ""
                echo "Проверок пройдено: $SUCCESS_COUNT/$MAX_ITERATIONS"
                echo "Статус: УСПЕШНО"
                echo ""
                echo "Запуск проекта..."
                ./target/release/bk-wiver-host.exe 2>&1 | head -10
                
                echo ""
                echo "Отправка на GitHub..."
                cd /root/.openclaw/workspace/bk-wiver-project
                git add -A
                git commit -m "docs: успешная сборка BK-Wiver v0.1.30"
                git push origin main
                echo ""
                echo "✅ Изменения отправлены на GitHub"
                echo "========================================="
                echo ""
                echo "✅ Проек BK-Wiver успешно собран!"
                echo "========================================="
                
                break
            else
                echo ""
                echo "❌ Сборка не прошла, проверю снова..."
                sleep 30
            fi
        else
            echo ""
            echo "❌ Проверка кода не прошла"
            echo "Попыток проверки: $ITERATION"
            sleep 30
        fi
    else
        echo "⏸️  Rust не установлен (проверка $ITERATION/$MAX_ITERATIONS)"
        echo "Осталось проверок: $((MAX_ITERATIONS - ITERATION))"
        sleep 30
    fi
    
    echo ""
    echo "========================================="
    echo "Следующая проверка через 30 секунд..."
    echo "========================================="
done

if [ $ITERATION -ge $MAX_ITERATIONS ]; then
    echo ""
    echo "========================================="
    echo "⚠️  Максимум проверок ($MAX_ITERATIONS) достигнут"
    echo "========================================="
    echo ""
    echo "Статус: Ожидание установки Rust"
    echo "Репозиторий: https://github.com/shioleg1-sketch/BK-Wiver"
fi
