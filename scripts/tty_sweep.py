# /// script
# requires-python = ">=3.10"
# dependencies = ["pyte"]
# ///
"""End-to-end TTY sweep for postui, runnable headless (no real terminal needed).

Runs the compiled binary on a pseudo-terminal, drives real keystrokes (and SGR
mouse sequences), renders the output through the pyte terminal emulator, and
asserts on the actual screen plus on-disk request files. A local HTTP server
provides /ok (JSON 200) and /slow (10 s delay, for the cancel flow) endpoints,
and a fake $EDITOR exercises the ctrl+e suspend/resume round-trip.

The harness answers the terminal-status queries the app emits (DSR cursor
position, device attributes, kitty keyboard probe) — without those replies,
ratatui's re-init after the external-editor suspend hangs on a headless PTY.

Usage:  cargo build -p postui && uv run scripts/tty_sweep.py
Exit code 0 = all checks passed. Uses an isolated XDG_CONFIG_HOME; never
touches your real ~/.config/postui.
"""
import os, pty, time, sys, fcntl, termios, struct, select, re, threading, json
import http.server
import tempfile
import pyte

SCRATCH = tempfile.mkdtemp(prefix="postui-tty-sweep-")
XDG = SCRATCH + "/xdg"
os.makedirs(XDG, exist_ok=True)

class H(http.server.BaseHTTPRequestHandler):
    def _send(self, code, body):
        b = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        if self.path.startswith("/slow"):
            time.sleep(10); self._send(200, '{"slow":true}')
        else:
            self._send(200, '{"ok":true,"items":[1,2,3],"nested":{"needle":"haystack"}}')
    def log_message(self, *a): pass

srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
PORT = srv.server_address[1]
threading.Thread(target=srv.serve_forever, daemon=True).start()

ED = SCRATCH + "/fake_editor.sh"
with open(ED, "w") as f:
    f.write('#!/bin/sh\nprintf \'{"edited": true}\' > "$1"\n')
os.chmod(ED, 0o755)

env = dict(os.environ)
env["XDG_CONFIG_HOME"] = XDG
env["TERM"] = "xterm-256color"
env["EDITOR"] = ED

pid, fd = pty.fork()
if pid == 0:
    os.execve("./target/debug/postui", ["postui"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

screen = pyte.Screen(120, 40)
stream = pyte.Stream(screen)

# Act like a real terminal: answer status queries the app sends.
DSR = re.compile(rb'\x1b\[6n')          # cursor position report
DA  = re.compile(rb'\x1b\[0?c')         # primary device attributes
KITTY = re.compile(rb'\x1b\[\?u')       # kitty keyboard query

def drain(t=0.8):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try: chunk = os.read(fd, 65536)
            except OSError: return
            if not chunk: return
            for _ in DSR.finditer(chunk):
                row, col = screen.cursor.y + 1, screen.cursor.x + 1
                os.write(fd, f"\x1b[{row};{col}R".encode())
            for _ in DA.finditer(chunk):
                os.write(fd, b"\x1b[?6c")
            for _ in KITTY.finditer(chunk):
                os.write(fd, b"\x1b[?0u")
            stream.feed(chunk.decode(errors="replace"))

def scr(): return "\n".join(screen.display)
def send(s, wait=0.4):
    os.write(fd, s.encode() if isinstance(s, str) else s)
    drain(wait)

results = []
def check(name, cond, dump=True):
    results.append((name, bool(cond)))
    print(("PASS " if cond else "FAIL ") + name)
    if not cond and dump:
        for line in screen.display:
            if line.strip(): print("  |" + line.rstrip())

drain(2.0)
check("launch: chrome renders", all(m in scr() for m in ["postui", "Requests", "Response"]), False)
send("n"); send("api/ping"); send("\r", 0.8)
p = XDG + "/postui/default/requests/api/ping.toml"
check("create api/ping request", os.path.exists(p), False)

url = f"http://127.0.0.1:{PORT}/ok"
send("\x1bu"); send(url, 0.6)
send("\x1b1", 0.3); send("\x1b[B", 0.3)
send("a"); send("debug"); send("\t"); send("yes"); send("\r", 0.4)
send("\x13", 0.6)
saved = open(p).read()
check("save: url + table-form params on disk", url in saved and 'debug = "yes"' in saved, False)

send("\x12", 2.5)
check("send: 200 pill + tree", "200" in scr() and '"items"' in scr())

send("\x10", 0.4); send("resp", 0.3); send("\r", 0.4)
send("/needle", 0.4); send("\r", 0.5)
check("search: counter + needle visible", re.search(r"\d+/\d+", scr()) and "needle" in scr(), False)

# collapse: cursor to the "items": [ container (line index 2 in tree; g then j j)
send("g", 0.3); send("j", 0.2); send("j", 0.2); send(" ", 0.5)
check("collapse: summary rendered", re.search(r"3 items|\[…", scr()))
send(" ", 0.3)

# cancel flow
send("\x10", 0.3); send("editor", 0.3); send("\r", 0.4)
send("\x1bu", 0.3)
for _ in range(len(url) + 5): os.write(fd, b"\x7f")
drain(0.4)
send(f"http://127.0.0.1:{PORT}/slow", 0.5)
send("\x12", 1.2)
check("in-flight: cancel hint", "cancel" in scr().lower(), False)
send("\x10", 0.3); send("resp", 0.3); send("\r", 0.4)
send("\x1b", 0.8)
check("esc cancels request", "cancelled" in scr().lower(), False)

# $EDITOR round-trip (now with DSR answered)
send("\x10", 0.3); send("editor", 0.3); send("\r", 0.4)
send("\x1b3", 0.4); send("\x1b[B", 0.3)
send("\x05", 2.5)
check("$EDITOR round-trip: body replaced", "edited" in scr())
check("app still healthy after $EDITOR (footer intact)", "commands" in scr(), False)

# wheel scroll on response pane
os.write(fd, b"\x1b[<65;60;30M"); drain(0.4)
alive = True
try: alive = os.waitpid(pid, os.WNOHANG) == (0, 0)
except ChildProcessError: alive = False
check("wheel scroll: app alive", alive, False)

# ctrl+c with palette open
send("\x10", 0.4); send("\x03", 1.0)
time.sleep(0.6)
ok_exit = False
try:
    pid2, status = os.waitpid(pid, os.WNOHANG)
    ok_exit = pid2 == pid and os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0
    if pid2 != pid: os.kill(pid, 9); os.waitpid(pid, 0)
except ChildProcessError:
    ok_exit = False
check("ctrl+c quits with palette open", ok_exit, False)

srv.shutdown()
fails = [n for n, ok in results if not ok]
print(f"\n{len(results)-len(fails)}/{len(results)} passed" + (f" — FAILS: {fails}" if fails else ""))
sys.exit(1 if fails else 0)
