-- Every session's windows: each window's blocks, and the folder of each terminal in its
-- tiled layout on this machine, in layout order. Floating layers come and go over a
-- window, as the picker itself does, so they place nothing. A session that closes
-- while it is read is left out.

local host_id = call("server.status", {}).host_id
local sessions = {}
for _, entry in ipairs(call("session.list", {}).sessions or {}) do
  local attached, err = rex.call("session.attach", { session_id = entry.session_id })
  if attached == nil then
    if code_of(err) ~= "session_not_found" then
      error(err, 0)
    end
  else
    local windows = {}
    for _, window in ipairs(attached.view.windows or {}) do
      local blocks, terminals = {}, {}
      for _, layer in ipairs(window.layers or {}) do
        for _, block in ipairs(layer.blocks or {}) do
          table.insert(blocks, block.block_id)
          if layer.kind == "tiled" and block.creator_name == TERMINAL
            and block.host_id == host_id then
            table.insert(terminals, terminal_folder(entry.session_id, block.block_id))
          end
        end
      end
      table.insert(windows, {
        window_id = window.window_id,
        blocks = blocks,
        terminals = terminals,
      })
    end
    table.insert(sessions, {
      session_id = entry.session_id,
      label = attached.view.label,
      windows = windows,
    })
  end
end
return { sessions = sessions }
