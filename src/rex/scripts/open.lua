-- Makes what `opening` asks for, if anything, then makes the window its session's
-- active one and asks the app `client_id` to show it. Returns the window.

-- How long a new session is left before the app is asked to show it.
local SETTLE_SECONDS = 0.05

local opening = rex.args.opening
local target
if opening.existing ~= nil then
  target = opening.existing
  attach(target.session_id)
elseif opening.new_window ~= nil then
  local request = opening.new_window
  attach(request.session_id)
  local made = call("session.new_window", request)
  target = { session_id = request.session_id, window_id = made.window_id }
elseif opening.new_session ~= nil then
  local request = opening.new_session
  local created = call("session.create", {
    label = request.label,
    initial_windows = {
      { window_label = request.window_label, layout = request.layout },
    },
  })
  local windows = created.initial_windows or {}
  if #windows ~= 1 then
    error("session.create made " .. #windows .. " windows, not the one asked for", 0)
  end
  target = { session_id = created.session_id, window_id = windows[1].window_id }
  attach(target.session_id)
  -- Rex.app takes around 200ms to show a session it was told of only moments before,
  -- but a few milliseconds once it has caught up, so the pause saves time.
  rex.sleep(SETTLE_SECONDS)
else
  error("nothing to open", 0)
end
call("session.focus_window", target)
call("client.perform", {
  client_id = rex.args.client_id,
  action = "session.select",
  args = target,
})
return target
