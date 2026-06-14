return function(__lux_import)
  --#lux source: ..\examples\gmod_project\src\server\init.lux:1
  local __lux_exports = {}
  --#lux source: ..\examples\gmod_project\src\server\init.lux:12
  local announce
  --#lux source: ..\examples\gmod_project\src\server\init.lux:1
  do
    local __lux_import_1 = __lux_import("gmod_project/hud#server")
    local buildHud = __lux_import_1.buildHud
  --#lux source: ..\examples\gmod_project\src\server\init.lux:2
    local __lux_import_2 = __lux_import("lux/gmod#server")
    local netx = __lux_import_2.netx
  --#lux source: ..\examples\gmod_project\src\server\init.lux:5
    netx.register("lux:announce_hud")
  --#lux source: ..\examples\gmod_project\src\server\init.lux:7
    do
      local __lux_macro_net_name_1 = "lux:request_hud"
      local __lux_macro_net_callback_2 = function(len, ply)
        local hud = buildHud(0, { ply })
        return print(hud.text)
      end
      do
        util.AddNetworkString(__lux_macro_net_name_1)
      end
      net.Receive(__lux_macro_net_name_1, __lux_macro_net_callback_2)
    end
  --#lux source: ..\examples\gmod_project\src\server\init.lux:12
    announce = function(players, ...)
      local hud = buildHud(0, players, ...)
      print(hud.text)
      return netx.broadcast(
        "lux:announce_hud",
        function()
          return net.WriteString(hud.text)
        end
      )
    end
  --#lux source: ..\examples\gmod_project\src\server\init.lux:1
  end
  
  --#lux source: ..\examples\gmod_project\src\server\init.lux:12
  __lux_exports.announce = announce
  
  --#lux source: ..\examples\gmod_project\src\server\init.lux:1
  return __lux_exports
end
