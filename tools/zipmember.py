"""Extracts one member of a remote zip with HTTP range requests (only the
member's bytes are fetched). usage: python3 -I tools/zipmember.py <url> <member>
<out file>; WORKFLOW "Chromium (BR)" (the snapshot's symbols)."""
import io, ssl, sys, urllib.request, zipfile

url, member, out = sys.argv[1:4]
ctx = ssl.create_default_context(cafile="/root/.ccr/ca-bundle.crt")
opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx))

class Remote(io.RawIOBase):
    def __init__(self):
        r = opener.open(urllib.request.Request(url, method="HEAD"))
        self.size = int(r.headers["Content-Length"]); self.pos = 0
    def seekable(self): return True
    def readable(self): return True
    def tell(self): return self.pos
    def seek(self, off, whence=0):
        self.pos = {0: off, 1: self.pos + off, 2: self.size + off}[whence]; return self.pos
    def readinto(self, b):
        if self.pos >= self.size: return 0
        end = min(self.pos + len(b), self.size) - 1
        for attempt in range(5):
            try:
                req = urllib.request.Request(url, headers={"Range": f"bytes={self.pos}-{end}"})
                data = opener.open(req, timeout=120).read(); break
            except Exception as e:
                if attempt == 4: raise
        b[:len(data)] = data; self.pos += len(data); return len(data)

f = io.BufferedReader(Remote(), buffer_size=8 << 20)
z = zipfile.ZipFile(f)
info = z.getinfo(member)
print(f"{member}: {info.file_size} bytes ({info.compress_size} compressed)", flush=True)
done = 0
with z.open(info) as src, open(out, "wb") as dst:
    while True:
        chunk = src.read(16 << 20)
        if not chunk: break
        dst.write(chunk); done += len(chunk)
        print(f"{done >> 20} MiB", flush=True)
print("done", flush=True)
