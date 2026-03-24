BK-Wiver Ubuntu server update package

How to use on server:
1. Copy the contents of this Update folder into /opt/bk-wiver
2. Do not overwrite /opt/bk-wiver/.env if it is already configured
3. Check /opt/bk-wiver/.env:
   SERVER_PUBLIC_URL=http://172.16.100.164:8080
4. Run:
   chmod +x /opt/bk-wiver/update-server-ubuntu.sh
   cd /opt/bk-wiver
   ./update-server-ubuntu.sh

What this update contains:
- current docker-compose files
- current server Rust sources
- session TTL fix
- signaling support for session.input_mouse and session.input_key

If the project is stored elsewhere, pass the path explicitly:
./update-server-ubuntu.sh /path/to/bk-wiver
