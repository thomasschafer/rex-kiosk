-- Helpers every rex-kiosk script starts with. Scripts run in one `rex do`, over one
-- connection, which is much quicker than a `rex` process per request.

local TERMINAL = "com.superlogical.terminal"

-- The server's error code at the end of a failed call's message, such as `not_found`.
local function code_of(err)
  return string.match(err, "code=([%w_]+)%)$")
end

-- Calls `method`, failing the script with the server's message if it fails.
local function call(method, args)
  local result, err = rex.call(method, args)
  if result == nil then
    error(err, 0)
  end
  return result
end

-- Starts receiving `session_id`'s events, which a connection must do before it can act
-- on a session or its blocks, and returns the session's view.
local function attach(session_id)
  return call("session.attach", { session_id = session_id }).view
end

-- Calls a method of a block that may have closed: nil if it has.
local function call_block(method, args)
  local result, err = rex.call(method, args)
  if result == nil and code_of(err) ~= "not_found" then
    error(err, 0)
  end
  return result
end

-- What a terminal knows of its folder: its foreground process's as `cwd`, or else the
-- one its shell last reported as `pwd`. Empty if neither is known or it has closed.
-- Its session must be attached.
local function terminal_folder(session_id, block_id)
  local args = { session_id = session_id, block_id = block_id, args = {} }
  local process = call_block(TERMINAL .. ".process", args)
  if process == nil then
    return {}
  end
  if process.foreground ~= nil and process.foreground.cwd ~= nil then
    return { cwd = process.foreground.cwd }
  end
  local reported = call_block(TERMINAL .. ".pwd", args)
  if reported == nil then
    return {}
  end
  return { pwd = reported.pwd }
end

