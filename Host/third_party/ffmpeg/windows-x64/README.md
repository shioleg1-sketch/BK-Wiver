Place the Windows x64 FFmpeg runtime here so the BK-Host installer can bundle it.

Expected files:
- ffmpeg.exe
- optional runtime DLL files if your FFmpeg build is not static

The Windows installer copies these files next to bk-wiver-host.exe.
At runtime, BK-Host should prefer the local ffmpeg.exe in its install directory.
