# Инструкции по добавлению SSH ключа в GitHub

## 📝 Ваши SSH ключи

**Приватный ключ:**
```bash
/root/.openclaw/workspace/ssh_git_key
```

**Публичный ключ:**
```bash
/root/.openclaw/workspace/ssh_git_key.pub
```

## 🔑 Добавление ключа в GitHub

### Шаг 1: Скопируйте публичный ключ

Скопируйте содержимое файла `/root/.openclaw/workspace/ssh_git_key.pub`:

```bash
cat /root/.openclaw/workspace/ssh_git_key.pub
```

**Ключ:**
```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL62gpgReP48GZxwoNJ8tBdFscJc/l4VC1kbMV/iV8me openclaw-workspace@bk-wiver.dev
```

### Шаг 2: Перейдите в настройки GitHub

1. Откройте браузер и перейдите на: https://github.com/settings/settings
2. В меню слева найдите **SSH and GPG keys** (в разделе Access)
3. Нажмите кнопку **New SSH key**
4. Заполните поля:
   - **Title**: "OpenClaw Workspace BK-Wiver"
   - **Key**: вставьте скопированный ключ
   - **Confirm add** (кнопка внизу)

### Шаг 3: Проверьте доступ

После добавления ключа, проверьте доступ к репозиторию:

```bash
cd /root/.openclaw/workspace/bk-wiver-project
git remote set-url origin git@github.com:shioleg1-sketch/BK-Wiver.git
git push
```

## ✅ После добавления ключа

Вы сможете:

- Pushить изменения в Git
- Pullить обновления
- Создавать pull requests
- Управлять ветками

## 🚀 Отправка изменений после добавления ключа

После добавления SSH ключа в GitHub:

1. **Проверьте доступ:**
   ```bash
   ssh -T git@github.com
   ```
   
   Вы должны увидеть сообщение:
   ```
   Hi shioleg1-sketch! You've successfully authenticated, but GitHub does not provide shell access.
   ```

2. **Отправьте изменения:**
   ```bash
   cd /root/.openclaw/workspace/bk-wiver-project
   git push -u origin main
   ```

## 📝 Команды для отправки изменений

```bash
# 1. Проверка статуса
cd /root/.openclaw/workspace/bk-wiver-project
git status

# 2. Отправка на GitHub
git push origin main

# 3. Проверка успешной отправки
git status

# 4. Если нужно добавить файл и отправить
git add <filename>
git commit -m "ваше сообщение"
git push origin main
```

## 🎯 Следующие шаги

1. **Добавьте SSH ключ в GitHub**
2. **Проверьте доступ:** `ssh -T git@github.com`
3. **Отправьте изменения:** `git push`
4. **Проверьте результат** на https://github.com/shioleg1-sketch/BK-Wiver

---

**Дата создания:** 2026-04-05  
**Статус:** В ожидании добавления ключа
