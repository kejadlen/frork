-- Fennel's binary.fnl from 1.6.0, compiled to Lua: the code
-- behind fennel --compile-binary. Not part of the library build vendored
-- alongside it. MIT licensed, like fennel-1.6.0.lua.
local fennel = require("fennel")
local _local_1_ = require("fennel.utils")
local warn = _local_1_.warn
local copy = _local_1_.copy
local function shellout(command)
  local f = io.popen(command)
  local stdout = f:read("*all")
  return (f:close() and stdout)
end
local function execute(cmd)
  local case_2_ = os.execute(cmd)
  if (case_2_ == 0) then
    return true
  elseif (case_2_ == true) then
    return true
  else
    return nil
  end
end
local function string__3ec_hex_literal(characters)
  local _4_
  do
    local tbl_26_ = {}
    local i_27_ = 0
    for character in characters:gmatch(".") do
      local val_28_ = ("0x%02x"):format(string.byte(character))
      if (nil ~= val_28_) then
        i_27_ = (i_27_ + 1)
        tbl_26_[i_27_] = val_28_
      else
      end
    end
    _4_ = tbl_26_
  end
  return table.concat(_4_, ", ")
end
local c_shim = "#ifdef __cplusplus\nextern \"C\" {\n#endif\n#include <lauxlib.h>\n#include <lua.h>\n#include <lualib.h>\n#ifdef __cplusplus\n}\n#endif\n#include <signal.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\n#if LUA_VERSION_NUM == 501\n  #define LUA_OK 0\n#endif\n\n/* Copied from lua.c */\n\nstatic lua_State *globalL = NULL;\n\nstatic void lstop (lua_State *L, lua_Debug *ar) {\n  (void)ar;  /* unused arg. */\n  lua_sethook(L, NULL, 0, 0);  /* reset hook */\n  luaL_error(L, \"interrupted!\");\n}\n\nstatic void laction (int i) {\n  signal(i, SIG_DFL); /* if another SIGINT happens, terminate process */\n  lua_sethook(globalL, lstop, LUA_MASKCALL | LUA_MASKRET | LUA_MASKCOUNT, 1);\n}\n\nstatic void createargtable (lua_State *L, char **argv, int argc, int script) {\n  int i, narg;\n  if (script == argc) script = 0;  /* no script name? */\n  narg = argc - (script + 1);  /* number of positive indices */\n  lua_createtable(L, narg, script + 1);\n  for (i = 0; i < argc; i++) {\n    lua_pushstring(L, argv[i]);\n    lua_rawseti(L, -2, i - script);\n  }\n  lua_setglobal(L, \"arg\");\n}\n\nstatic int msghandler (lua_State *L) {\n  const char *msg = lua_tostring(L, 1);\n  if (msg == NULL) {  /* is error object not a string? */\n    if (luaL_callmeta(L, 1, \"__tostring\") &&  /* does it have a metamethod */\n        lua_type(L, -1) == LUA_TSTRING)  /* that produces a string? */\n      return 1;  /* that is the message */\n    else\n      msg = lua_pushfstring(L, \"(error object is a %%s value)\",\n                            luaL_typename(L, 1));\n  }\n  /* Call debug.traceback() instead of luaL_traceback() for Lua 5.1 compat. */\n  lua_getglobal(L, \"debug\");\n  lua_getfield(L, -1, \"traceback\");\n  /* debug */\n  lua_remove(L, -2);\n  lua_pushstring(L, msg);\n  /* original msg */\n  lua_remove(L, -3);\n  lua_pushinteger(L, 2);  /* skip this function and traceback */\n  lua_call(L, 2, 1); /* call debug.traceback */\n  return 1;  /* return the traceback */\n}\n\nstatic int docall (lua_State *L, int narg, int nres) {\n  int status;\n  int base = lua_gettop(L) - narg;  /* function index */\n  lua_pushcfunction(L, msghandler);  /* push message handler */\n  lua_insert(L, base);  /* put it under function and args */\n  globalL = L;  /* to be available to 'laction' */\n  signal(SIGINT, laction);  /* set C-signal handler */\n  status = lua_pcall(L, narg, nres, base);\n  signal(SIGINT, SIG_DFL); /* reset C-signal handler */\n  lua_remove(L, base);  /* remove message handler from the stack */\n  return status;\n}\n\nint main(int argc, char *argv[]) {\n lua_State *L = luaL_newstate();\n luaL_openlibs(L);\n createargtable(L, argv, argc, 0);\n\n static const unsigned char lua_loader_program[] = {\n%s\n};\n  if(luaL_loadbuffer(L, (const char*)lua_loader_program,\n                     sizeof(lua_loader_program), \"%s\") != LUA_OK) {\n    fprintf(stderr, \"luaL_loadbuffer: %%s\\n\", lua_tostring(L, -1));\n    lua_close(L);\n    return 1;\n  }\n\n  /* lua_bundle */\n  lua_newtable(L);\n  static const unsigned char lua_require_1[] = {\n  %s\n  };\n  lua_pushlstring(L, (const char*)lua_require_1, sizeof(lua_require_1));\n  lua_setfield(L, -2, \"%s\");\n\n%s\n\n  if (docall(L, 1, LUA_MULTRET)) {\n    const char *errmsg = lua_tostring(L, 1);\n    if (errmsg) {\n      fprintf(stderr, \"%%s\\n\", errmsg);\n    }\n    lua_close(L);\n    return 1;\n  }\n  lua_close(L);\n  return 0;\n}"
local function file__3elua(filename, options)
  local f
  if (filename == "-") then
    f = io.stdin
  else
    f = assert(io.open(filename, "rb"))
  end
  local lua_code
  if filename:find("%.luac?$") then
    lua_code = f:read("*a")
  else
    lua_code = fennel["compile-string"](f:read("*a"), options)
  end
  f:close()
  return lua_code
end
local function module_name(open, rename, used_renames)
  local require_name
  do
    local case_8_ = rename[open]
    if (nil ~= case_8_) then
      local renamed = case_8_
      used_renames[open] = true
      require_name = renamed
    else
      local _ = case_8_
      require_name = open
    end
  end
  return (require_name:sub(1, 1) .. require_name:sub(2):gsub("_", "."))
end
local function native_loader(native, _3foptions)
  local opts = (_3foptions or {["rename-modules"] = {}})
  local rename = (opts["rename-modules"] or {})
  local used_renames = {}
  local nm = (os.getenv("NM") or "nm")
  local out = {"  /* native libraries */"}
  for _, path in ipairs(native) do
    local opens = {}
    for open in shellout((nm .. " " .. path)):gmatch("[^dDt] _?luaopen_([%a%p%d]+)") do
      table.insert(opens, open)
    end
    if (nil == opens[1]) then
      warn(("Native module %s did not contain any luaopen_* symbols. " .. "Did you mean to use --native-library instead of --native-module?"):format(path))
    else
    end
    for _0, open in ipairs(opens) do
      table.insert(out, ("  int luaopen_%s(lua_State *L);"):format(open))
      table.insert(out, ("  lua_pushcfunction(L, luaopen_%s);"):format(open))
      table.insert(out, ("  lua_setfield(L, -2, \"%s\");\n"):format(module_name(open, rename, used_renames)))
    end
  end
  for key, val in pairs(rename) do
    if not used_renames[key] then
      warn(("unused --rename-native-module %s %s argument. " .. "Did you mean to include a native module?"):format(key, val))
    else
    end
  end
  return table.concat(out, "\n")
end
local function file__3ec(filename, native, options)
  local basename = filename:gsub("(.*[\\/])(.*)", "%2")
  local basename_noextension = (basename:match("(.+)%.") or basename)
  local dotpath = filename:gsub("^%.%/", ""):gsub("[\\/]", ".")
  local dotpath_noextension = (dotpath:match("(.+)%.") or dotpath)
  local fennel_loader
  local _12_
  do
    _12_ = "(do (local bundle_2_auto ...) (fn loader_3_auto [name_4_auto] (case (or (. bundle_2_auto name_4_auto) (. bundle_2_auto (.. name_4_auto \".init\"))) (where mod_5_auto (= \"function\" (type mod_5_auto))) mod_5_auto (where mod_5_auto (= \"string\" (type mod_5_auto))) (assert (if (= _VERSION \"Lua 5.1\") (loadstring mod_5_auto name_4_auto) (load mod_5_auto name_4_auto))) nil (values nil (: \"\n\\tmodule '%%s' not found in fennel bundle\" \"format\" name_4_auto)))) (table.insert (or package.loaders package.searchers) 2 loader_3_auto) ((assert (loader_3_auto \"%s\")) ((or unpack table.unpack) arg)))"
  end
  fennel_loader = _12_:format(dotpath_noextension)
  local lua_loader = fennel["compile-string"](fennel_loader)
  local rename_modules = options["rename-modules"]
  return c_shim:format(string__3ec_hex_literal(lua_loader), basename_noextension, string__3ec_hex_literal(file__3elua(filename, options)), dotpath_noextension, native_loader(native, {["rename-modules"] = rename_modules}))
end
local function write_c(filename, native, options)
  local out_filename = (filename .. "_binary.c")
  local f = assert(io.open(out_filename, "w+"))
  f:write(file__3ec(filename, native, options))
  f:close()
  return out_filename
end
local function compile_binary(lua_c_path, executable_name, static_lua, lua_include_dir, native)
  local cc = (os.getenv("CC") or "cc")
  local rdynamic, bin_extension, ldl_3f
  local _13_
  do
    local tmp_3_ = shellout((cc .. " -dumpmachine"))
    if (nil ~= tmp_3_) then
      _13_ = tmp_3_:match("mingw")
    else
      _13_ = nil
    end
  end
  if _13_ then
    rdynamic, bin_extension, ldl_3f = "", ".exe", false
  else
    rdynamic, bin_extension, ldl_3f = "-rdynamic", "", true
  end
  local compile_command
  local _16_
  if ldl_3f then
    _16_ = "-ldl"
  else
    _16_ = ""
  end
  compile_command = {cc, "-Os", lua_c_path, table.concat(native, " "), static_lua, rdynamic, "-lm", _16_, "-o", (executable_name .. bin_extension), "-I", lua_include_dir, os.getenv("CC_OPTS")}
  if os.getenv("FENNEL_DEBUG") then
    print("Compiling with", table.concat(compile_command, " "))
  else
  end
  if not execute(table.concat(compile_command, " ")) then
    print("Failed:", table.concat(compile_command, " "))
    print("Ensure CC is set to the C compiler you intend to use.")
    os.exit(1)
  else
  end
  if not os.getenv("FENNEL_DEBUG") then
    os.remove(lua_c_path)
  else
  end
  return os.exit(0)
end
local function native_path_3f(path)
  local extension, version_extension = path:match("%.(%a+)(%.?%d*)$")
  if (version_extension and (version_extension ~= "") and not version_extension:match("%.%d+")) then
    return false
  else
    if (extension == "a") then
      return path
    elseif (extension == "o") then
      return path
    elseif (extension == "so") then
      return path
    elseif (extension == "dylib") then
      return path
    else
      local _ = extension
      return false
    end
  end
end
local function extract_native_args(args)
  local native = {modules = {}, libraries = {}, ["rename-modules"] = {}}
  for i = #args, 1, -1 do
    if ("--native-module" == args[i]) then
      local path = assert(native_path_3f(table.remove(args, (i + 1))))
      table.insert(native.modules, 1, path)
      table.insert(native.libraries, 1, path)
      table.remove(args, i)
    else
    end
    if ("--native-library" == args[i]) then
      table.insert(native.libraries, 1, assert(native_path_3f(table.remove(args, (i + 1)))))
      table.remove(args, i)
    else
    end
    if ("--rename-native-module" == args[i]) then
      local original = table.remove(args, (i + 1))
      local new = table.remove(args, (i + 1))
      native["rename-modules"][original] = new
      table.remove(args, i)
    else
    end
  end
  if next(args) then
    print(table.concat(args, " "))
    error(("Unknown args: " .. table.concat(args, " ")))
  else
  end
  return native
end
local function compile(filename, executable_name, static_lua, lua_include_dir, options, args)
  local _let_27_ = extract_native_args(args)
  local modules = _let_27_.modules
  local libraries = _let_27_.libraries
  local rename_modules = _let_27_["rename-modules"]
  local opts = {["rename-modules"] = rename_modules}
  copy(options, opts)
  return compile_binary(write_c(filename, modules, opts), executable_name, static_lua, lua_include_dir, libraries)
end
local help = "\nUsage: %s --compile-binary FILE OUT STATIC_LUA_LIB LUA_INCLUDE_DIR\n\nCompile a binary from your Fennel program.\n\nRequires a C compiler, a copy of liblua, and Lua's dev headers. Implies\nthe --require-as-include option.\n\n  FILE: the source being compiled; either Fennel or Lua.\n  OUT: the name of the executable to generate\n  STATIC_LUA_LIB: the path to the Lua library to use in the executable\n  LUA_INCLUDE_DIR: the path to the directory of Lua C header files\n\nFor example, on a Debian system, to compile a file called program.fnl using\nLua 5.4, you would use this:\n\n    $ %s --compile-binary program.fnl program \\\n        /usr/lib/x86_64-linux-gnu/liblua5.4.a /usr/include/lua5.4\n\nThe program will be compiled to Lua, (unless it's already Lua) then compiled to\nC, then compiled to machine code. You can set the CC environment variable to\nchange the compiler used (default: cc) or set CC_OPTS to pass in compiler\noptions. For example set CC_OPTS=-static to generate a binary with static\nlinking.\n\nThis method is currently limited to programs do not transitively require Lua\nmodules. Requiring a Lua module directly will work, but requiring a Lua module\nwhich requires another will fail.\n\nTo include C libraries that contain Lua modules, add --native-module path/to.so,\nand to include C libraries without modules, use --native-library path/to.so.\nThese options are unstable, barely tested, and even more likely to break.\n\nIf you need to change the require name that a given native module is referenced\nas, you can use the --rename-native-module ORIGINAL NEW. ORIGINAL should be the\nsuffix of the luaopen_* symbol in the native module. NEW should be the string\nyou wish to pass to require to require the given native module. This can be used\nto handle cases where the name of an object file does not match the name of the\nluaopen_* symbol(s) within it. For example, the Lua readline bindings include a\nreadline.lua file which is usually required as \"readline\", and a C-readline.so\nfile which is required in the Lua half of the bindings like so:\n\n    require 'C-readline'\n\nHowever, the symbol within the C-readline.so file is named luaopen_readline, so\nby default --compile-binary will make it so you can require it as \"readline\",\nwhich collides with the name of the readline.lua file and doesn't match the\nrequire call within readline.lua. In order to include the module within your\ncompiled binary and have it get picked up by readline.lua correctly, you can\nspecify the name used to refer to it in a require call by compiling it like\nso (this is assuming that program.fnl requires the Lua bindings):\n\n    $ %s --compile-binary program.fnl program \\\n        /usr/lib/x86_64-linux-gnu/liblua5.4.a /usr/include/lua5.4 \\\n        --native-module C-readline.so \\\n        --rename-native-module readline C-readline\n"
return {compile = compile, help = help}
