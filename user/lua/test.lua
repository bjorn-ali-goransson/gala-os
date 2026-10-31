-- The `lua` scenario's script (user/lua/lua.sh): a Lua built on gala by
-- busybox's make and TinyCC runs it. One labelled line per result.

-- Strings.
local s = "Hello, gala"
print("upper: " .. s:upper() .. " len=" .. #s)
print("gsub: " .. (s:gsub("l", "L")))
print("find: " .. table.concat({s:find("gala")}, ","))
print("rep: " .. ("ab"):rep(3, "-"))
local words = {}
for w in ("one two three"):gmatch("%a+") do words[#words + 1] = w:reverse() end
print("gmatch: " .. table.concat(words, " "))

-- Tables and sorting.
local t = {5, 3, 9, 1, 7}
table.sort(t)
print("sort: " .. table.concat(t, " "))
table.sort(t, function(a, b) return a > b end)
print("sort desc: " .. table.concat(t, " "))
local counts = {}
for _, w in ipairs({"a", "b", "a", "c", "a", "b"}) do counts[w] = (counts[w] or 0) + 1 end
print(string.format("counts: a=%d b=%d c=%d", counts.a, counts.b, counts.c))

-- Closures.
local function counter()
  local n = 0
  return function() n = n + 1; return n end
end
local c1, c2 = counter(), counter()
c1(); c1()
print("closures: " .. c1() .. " " .. c2())

-- Numbers: integers, floats and their formatting.
print("int: " .. 7 // 2 .. " " .. 7 % 3 .. " " .. 2^10 .. " " .. math.maxinteger)
print("float: " .. 1 / 3 .. " " .. 0.1 + 0.2 .. " " .. 1e100 .. " " .. 2.5e-5)
print(string.format("format: %.3f %5.1f %e %g %x", math.pi, 2.26, 12345.678, 0.0001, 255))
print("tonumber: " .. tonumber("0x10") .. " " .. tonumber("1e2") .. " " .. tostring(tonumber("z")))
print("math: " .. math.floor(-3.5) .. " " .. math.ceil(3.2) .. " " .. math.abs(-4) .. " " .. math.sqrt(16))

-- Errors.
local ok, err = pcall(function() error("boom", 0) end)
print("pcall: " .. tostring(ok) .. " " .. err)
ok, err = pcall(function() error({code = 42}) end)
print("error table: " .. err.code)
ok, err = pcall(function() local x = nil; return x.y end)
print("runtime error: " .. tostring(ok) .. " " .. (err:match("attempt to index a nil value") or err))

-- Coroutines.
local co = coroutine.create(function(a)
  local b = coroutine.yield(a * 2)
  return a + b
end)
local _, first = coroutine.resume(co, 5)
local _, second = coroutine.resume(co, 10)
print("coroutine: " .. first .. " " .. second .. " " .. coroutine.status(co))

-- Files.
local name = os.getenv("TEMP") .. "/gala_lua.txt"
local f = assert(io.open(name, "w"))
f:write("line 1\n", "line 2\n")
f:close()
local lines = 0
for _ in io.lines(name) do lines = lines + 1 end
print("io: " .. lines .. " lines")
assert(os.remove(name))

print("lua: ok")
