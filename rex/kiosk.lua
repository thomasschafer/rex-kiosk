-- rex-kiosk for Rex: an action that opens the picker in a floating pane over the
-- current window. Put this file beside your init.lua and add:
--
--   require("kiosk").setup()
--   rex.bind("cmd+p", "kiosk")
--
-- setup takes an optional table:
--   command  the program to run and any arguments before kiosk's own, as a list.
--            Defaults to { "rex-kiosk" }, found on the PATH your login shell sets.
--   bounds   where the pane floats, as { x, y, w, h } fractions of the window.

local M = {}

local SHELL = "com.superlogical.terminal.shell"

function M.setup(opts)
  opts = opts or {}
  local base = opts.command or { "rex-kiosk" }
  local bounds = opts.bounds or { 0.1, 0.1, 0.8, 0.8 }

  rex.action{
    name = "kiosk",
    title = "Open Repository or Branch…",
    description = "Fuzzy-find a Git repository, branch or folder with rex-kiosk and open it",
    keywords = "kiosk repository repo branch worktree folder project picker",
    run = function(ctx)
      local command = {}
      for _, word in ipairs(base) do
        table.insert(command, word)
      end
      -- The app to show what opens in, and the pane whose folder to start from.
      if ctx.client_id ~= nil then
        table.insert(command, "--client")
        table.insert(command, ctx.client_id)
      end
      if ctx.block_id ~= nil then
        table.insert(command, "--block")
        table.insert(command, ctx.block_id)
      end
      local _, err = rex.session.new_layer{
        bounds = bounds,
        focus = true,
        layout = rex.layout.block{ flavor = SHELL, options = { command = command } },
      }
      if err ~= nil then
        error(err, 0)
      end
    end,
  }
end

return M
