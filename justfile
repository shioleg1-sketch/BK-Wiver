set shell := ["sh", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

default:
  @just --list

up:
  docker compose up --build

up-detached:
  docker compose up --build -d

bootstrap:
  @echo "Create .env from .env.example if you need custom ports or credentials."
  @echo "Then run: just up"

down:
  docker compose down

restart:
  docker compose down
  docker compose up --build -d

logs:
  docker compose logs -f server postgres

ps:
  docker compose ps

check:
  cargo check -p bk-wiver-server

check-desktop:
  cargo check -p bk-wiver-desktop

run-desktop:
  cargo run -p bk-wiver-desktop

build-desktop-release:
  cargo build -p bk-wiver-desktop --release

package-desktop-installer:
  "C:\Users\oleg\AppData\Local\Programs\Inno Setup 6\ISCC.exe" "deploy\windows\BK-Wiver-Desktop.iss"

fmt:
  cargo fmt

reset-db:
  docker compose down -v

rebuild:
  docker compose build --no-cache

server-logs:
  docker compose logs -f server

postgres-logs:
  docker compose logs -f postgres

up-lan:
  docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml up --build -d

logs-lan:
  docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml logs -f server postgres

ps-lan:
  docker compose -f docker-compose.yml -f deploy/ubuntu/docker-compose.lan.yml ps
