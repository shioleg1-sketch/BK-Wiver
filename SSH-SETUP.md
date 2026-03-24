# Настройка SSH для GitHub

## Дата: 2026-03-24

---

## ✅ Выполненная настройка

### 1. SSH ключ

**Ключ:** `~/.ssh/bk-wiver`

**Публичный ключ:** `~/.ssh/bk-wiver.pub`

**Отпечаток:** `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOSK+tFK7JghgC+Z7WthpbfW+ftujB3MZEFkl7X6dTNf`

### 2. Конфигурация SSH

**Файл:** `~/.ssh/config`

```ssh
Host github.com
    HostName github.com
    User git
    IdentityFile ~/.ssh/bk-wiver
    IdentitiesOnly yes
    AddKeysToAgent yes
```

### 3. Автозапуск ssh-agent

**Файл:** `~/.bashrc`

Добавлено в конец файла:

```bash
# Auto-start ssh-agent and add bk-wiver key
if [ -z "$SSH_AUTH_SOCK" ]; then
    eval $(ssh-agent -s)
    ssh-add ~/.ssh/bk-wiver 2>/dev/null
fi
```

### 4. Права доступа

```bash
chmod 600 ~/.ssh/bk-wiver      # Приватный ключ
chmod 644 ~/.ssh/bk-wiver.pub  # Публичный ключ
chmod 600 ~/.ssh/config        # Конфигурация
```

---

## 🔍 Проверка

### Тест подключения

```bash
ssh -T git@github.com
```

**Ожидаемый результат:**
```
Hi shioleg1-sketch! You've successfully authenticated, but GitHub does not provide shell access.
```

### Проверка ключа

```bash
cat ~/.ssh/bk-wiver.pub
```

**Ожидаемый результат:**
```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOSK+tFK7JghgC+Z7WthpbfW+ftujB3MZEFkl7X6dTNf qwen-code@bk-wiver
```

### Проверка конфигурации

```bash
cat ~/.ssh/config
```

---

## 🚀 Использование

### Git команды работают автоматически

```bash
cd /opt/bk-wiver

# Pull
git pull origin main

# Push
git push origin main

# Status
git status
```

### Если ssh-agent не запущен

```bash
# Запустить агент и добавить ключ
eval $(ssh-agent -s)
ssh-add ~/.ssh/bk-wiver

# Теперь git команды будут работать
git push origin main
```

---

## 🐛 Решение проблем

### Проблема: Permission denied (publickey)

**Решение 1: Проверить ssh-agent**

```bash
# Проверить, запущен ли агент
echo $SSH_AUTH_SOCK

# Если пусто, запустить
eval $(ssh-agent -s)

# Добавить ключ
ssh-add ~/.ssh/bk-wiver
```

**Решение 2: Проверить права доступа**

```bash
chmod 700 ~/.ssh
chmod 600 ~/.ssh/bk-wiver
chmod 600 ~/.ssh/config
```

**Решение 3: Проверить конфигурацию**

```bash
cat ~/.ssh/config
```

Должно содержать:
```
Host github.com
    IdentityFile ~/.ssh/bk-wiver
    IdentitiesOnly yes
```

### Проблема: Key passphrase

Если ключ защищён паролем:

```bash
# Добавить ключ с паролем в агент
ssh-add ~/.ssh/bk-wiver

# Ввести пароль при запросе
```

Для ключа без пароля (текущая конфигурация):
```bash
# Просто добавить ключ
ssh-add ~/.ssh/bk-wiver
```

### Проблема: Known hosts

Если изменились ключи сервера GitHub:

```bash
# Удалить старые записи
ssh-keygen -R github.com

# Добавить заново при следующем подключении
ssh -T git@github.com
```

---

## 📋 Чеклист настройки нового сервера

1. [ ] Создать SSH ключ:
   ```bash
   ssh-keygen -t ed25519 -f ~/.ssh/bk-wiver -C "comment@server"
   ```

2. [ ] Добавить публичный ключ в GitHub:
   - Settings → SSH and GPG keys → New SSH key
   - Скопировать содержимое `~/.ssh/bk-wiver.pub`

3. [ ] Создать конфигурацию:
   ```bash
   cat > ~/.ssh/config << 'EOF'
   Host github.com
       HostName github.com
       User git
       IdentityFile ~/.ssh/bk-wiver
       IdentitiesOnly yes
       AddKeysToAgent yes
   EOF
   ```

4. [ ] Установить права:
   ```bash
   chmod 700 ~/.ssh
   chmod 600 ~/.ssh/bk-wiver
   chmod 644 ~/.ssh/bk-wiver.pub
   chmod 600 ~/.ssh/config
   ```

5. [ ] Проверить подключение:
   ```bash
   ssh -T git@github.com
   ```

6. [ ] Добавить автозапуск в ~/.bashrc (опционально)

---

## 🔐 Безопасность

**Текущая конфигурация:**
- Ключ без пароля (для автоматизации)
- Доступ только для пользователя root
- IdentitiesOnly=yes (используется только указанный ключ)

**Рекомендации:**
- Не копировать приватный ключ на другие сервера
- При компрометации — удалить ключ из GitHub и создать новый
- Регулярно обновлять ключи (раз в 6-12 месяцев)

---

## 📞 Поддержка

При проблемах с доступом:

1. Проверьте логи:
   ```bash
   ssh -vT git@github.com 2>&1 | tail -20
   ```

2. Проверьте ключ в GitHub:
   - https://github.com/settings/keys

3. Пересоздайте ключ при необходимости
