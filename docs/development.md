# Разработка BK-Wiver

## Кодировка и русский текст

Во всем проекте используется `UTF-8`.

Что уже зафиксировано в репозитории:

- [`.editorconfig`](/C:/BK-Wiver/.editorconfig) задает `charset = utf-8`;
- [`.gitattributes`](/C:/BK-Wiver/.gitattributes) фиксирует нормализацию текстовых файлов;
- [`.vscode/settings.json`](/C:/BK-Wiver/.vscode/settings.json) задает `files.encoding = utf8`.

Практическое правило:

- в PowerShell читать файлы с `-Encoding UTF8`;
- не сохранять исходники в `CP1251`, `UTF-8 with BOM` или `UTF-16`, если для этого нет отдельной причины;
- для документов и контрактов использовать `LF`, для `*.ps1` оставить `CRLF`.

Примеры:

```powershell
Get-Content README.md -Encoding UTF8
Get-Content docs\architecture.md -Encoding UTF8
```

## Структура

Сейчас в репозитории есть:

- `docs/` для архитектуры и процесса разработки;
- `proto/` для контрактов control plane.

Следующий технический шаг:

- описать API `bk-broker`;
- добавить схему `device inventory`;
- определить отдельные сообщения для file transfer и input events.
