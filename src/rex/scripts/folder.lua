-- What the terminal `block_id` of `session_id` knows of its folder.

attach(rex.args.session_id)
return terminal_folder(rex.args.session_id, rex.args.block_id)
