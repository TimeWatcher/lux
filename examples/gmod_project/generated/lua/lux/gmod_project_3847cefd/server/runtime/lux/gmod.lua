return function(__lux_import)
  local __lux_exports = {}
  local valid
  local hookx
  local timerx
  local netx
  local json
  local players
  local entsx
  local color
  local vgui
  local timerIdCounter
  do
    valid = {}
    hookx = {}
    timerx = {}
    netx = {}
    json = {}
    players = {}
    entsx = {}
    color = {}
    vgui = {}
    timerIdCounter = 0
    function valid.is(value)
      local __lux_tmp_1 = value ~= nil
      if __lux_tmp_1 then
        __lux_tmp_1 = IsValid(value)
      end
      return __lux_tmp_1
    end
    function valid.orNil(value)
      if valid.is(value) then
        return value
      end
      return nil
    end
    function valid.orFallback(value, fallback)
      if valid.is(value) then
        return value
      end
      return fallback
    end
    function valid.all(values)
      for index = 1, #values do
        if not valid.is(values[index]) then
          return false
        end
      end
      return true
    end
    function valid.any(values)
      for index = 1, #values do
        if valid.is(values[index]) then
          return true
        end
      end
      return false
    end
    function valid.filterInto(values, out)
      for index = #out, 1, -1 do
        out[index] = nil
      end
      local outIndex = 1
      for index = 1, #values do
        local value = values[index]
        if valid.is(value) then
          out[outIndex] = value
          outIndex = outIndex + 1
        end
      end
      return out
    end
    function hookx.handle(event, id)
      return { event = event, id = id }
    end
    function hookx.add(event, id, callback)
      hook.Add(event, id, callback)
      return hookx.handle(event, id)
    end
    function hookx.remove(handleOrEvent, id)
      local __lux_tmp_2 = id == nil
      if __lux_tmp_2 then
        __lux_tmp_2 = type(handleOrEvent) == "table"
      end
      if __lux_tmp_2 then
        hook.Remove(handleOrEvent.event, handleOrEvent.id)
        return true
      end
      hook.Remove(handleOrEvent, id)
      return true
    end
    function hookx.replace(event, id, callback)
      hook.Remove(event, id)
      hook.Add(event, id, callback)
      return hookx.handle(event, id)
    end
    function hookx.once(event, id, callback)
      local wrapped
      wrapped = function(...)
        hook.Remove(event, id)
        return callback(...)
      end
      hook.Add(event, id, wrapped)
      return hookx.handle(event, id)
    end
    function hookx.object(event, object, callback)
      hook.Add(event, object, callback)
      return hookx.handle(event, object)
    end
    function hookx.scoped(owner, event, callback)
      hook.Add(event, owner, callback)
      return hookx.handle(event, owner)
    end
    function timerx.id(prefix)
      if prefix == nil then
        prefix = "lux.timer"
      end
      timerIdCounter = timerIdCounter + 1
      return prefix .. "." .. tostring(timerIdCounter)
    end
    function timerx.handle(id)
      return { id = id }
    end
    function timerx.after(delay, callback, id)
      local timerId
      do
        local __lux_tmp_3 = id
        if __lux_tmp_3 == nil then
          __lux_tmp_3 = timerx.id("lux.timer.once")
        end
        timerId = __lux_tmp_3
      end
      timer.Create(timerId, delay, 1, callback)
      return timerx.handle(timerId)
    end
    function timerx.every(interval, callback, repetitions, id)
      if repetitions == nil then
        repetitions = 0
      end
      local timerId
      do
        local __lux_tmp_4 = id
        if __lux_tmp_4 == nil then
          __lux_tmp_4 = timerx.id("lux.timer.every")
        end
        timerId = __lux_tmp_4
      end
      timer.Create(timerId, interval, repetitions, callback)
      return timerx.handle(timerId)
    end
    function timerx.cancel(handleOrId)
      local timerId = handleOrId
      if type(handleOrId) == "table" then
        timerId = handleOrId.id
      end
      if timerId ~= nil then
        timer.Remove(timerId)
        return true
      end
      return false
    end
    function timerx.exists(handleOrId)
      local timerId = handleOrId
      if type(handleOrId) == "table" then
        timerId = handleOrId.id
      end
      local __lux_tmp_5 = timerId ~= nil
      if __lux_tmp_5 then
        __lux_tmp_5 = timer.Exists(timerId)
      end
      return __lux_tmp_5
    end
    function timerx.simple(delay, callback)
      timer.Simple(delay, callback)
      return nil
    end
    function netx.register(name)
      do
        util.AddNetworkString(name)
      end
      return name
    end
    function netx.receive(name, callback)
      net.Receive(name, callback)
      return name
    end
    function netx.start(name)
      net.Start(name)
      return name
    end
    function netx.broadcast(name, writer)
      net.Start(name)
      if writer ~= nil then
        writer()
      end
      net.Broadcast()
      return name
    end
    function netx.sendTo(name, target, writer)
      net.Start(name)
      if writer ~= nil then
        writer(target)
      end
      net.Send(target)
      return name
    end
    function netx.sendOmit(name, omitted, writer)
      net.Start(name)
      if writer ~= nil then
        writer(omitted)
      end
      net.SendOmit(omitted)
      return name
    end
    function netx.withMessage(name, writer)
      net.Start(name)
      if writer ~= nil then
        writer()
      end
      return name
    end
    function json.encode(value, pretty)
      if pretty == nil then
        pretty = false
      end
      return util.TableToJSON(value, pretty)
    end
    function json.decode(value)
      return util.JSONToTable(value)
    end
    function json.decodeOrNil(value)
      local ok, result = pcall(util.JSONToTable, value)
      if ok then
        return result
      end
      return nil
    end
    function players.all()
      return player.GetAll()
    end
    function players.findBySteamID64(steamId64)
      local allPlayers = player.GetAll()
      for index = 1, #allPlayers do
        local ply = allPlayers[index]
        local __lux_tmp_6 = valid.is(ply)
        if __lux_tmp_6 then
          __lux_tmp_6 = ply:SteamID64() == steamId64
        end
        if __lux_tmp_6 then
          return ply
        end
      end
      return nil
    end
    function players.aliveInto(out)
      for index = #out, 1, -1 do
        out[index] = nil
      end
      local allPlayers = player.GetAll()
      local outIndex = 1
      for index = 1, #allPlayers do
        local ply = allPlayers[index]
        local __lux_tmp_7 = valid.is(ply)
        if __lux_tmp_7 then
          __lux_tmp_7 = ply:Alive()
        end
        if __lux_tmp_7 then
          out[outIndex] = ply
          outIndex = outIndex + 1
        end
      end
      return out
    end
    function players.humansInto(out)
      for index = #out, 1, -1 do
        out[index] = nil
      end
      local allPlayers = player.GetHumans()
      for index = 1, #allPlayers do
        out[index] = allPlayers[index]
      end
      return out
    end
    function entsx.byClassInto(className, out)
      for index = #out, 1, -1 do
        out[index] = nil
      end
      local found = ents.FindByClass(className)
      for index = 1, #found do
        out[index] = found[index]
      end
      return out
    end
    function entsx.nearbyInto(origin, radius, out, className)
      for index = #out, 1, -1 do
        out[index] = nil
      end
      local found = ents.FindInSphere(origin, radius)
      local outIndex = 1
      for index = 1, #found do
        local ent = found[index]
        local __lux_tmp_8 = className == nil
        if not __lux_tmp_8 then
          __lux_tmp_8 = ent:GetClass() == className
        end
        if __lux_tmp_8 then
          out[outIndex] = ent
          outIndex = outIndex + 1
        end
      end
      return out
    end
    function color.rgb(r, g, b)
      return Color(r, g, b)
    end
    function color.rgba(r, g, b, a)
      return Color(r, g, b, a)
    end
    function color.byte(value)
      local rounded = math.floor(value + 0.5)
      if rounded < 0 then
        return 0
      end
      if rounded > 255 then
        return 255
      end
      return rounded
    end
    function color.copy(input)
      return Color(input.r, input.g, input.b, input.a)
    end
    function color.withAlpha(input, alpha)
      return Color(input.r, input.g, input.b, alpha)
    end
    function color.lerp(t, from, to)
      local fromA
      do
        local __lux_tmp_9 = from.a
        if __lux_tmp_9 == nil then
          __lux_tmp_9 = 255
        end
        fromA = __lux_tmp_9
      end
      local toA
      do
        local __lux_tmp_10 = to.a
        if __lux_tmp_10 == nil then
          __lux_tmp_10 = 255
        end
        toA = __lux_tmp_10
      end
      return Color(
        color.byte(from.r + (to.r - from.r) * t),
        color.byte(from.g + (to.g - from.g) * t),
        color.byte(from.b + (to.b - from.b) * t),
        color.byte(fromA + (toA - fromA) * t)
      )
    end
    function color.toHex(input)
      local __lux_tmp_11 = input.a
      if __lux_tmp_11 == nil then
        __lux_tmp_11 = 255
      end
      return string.format("%02x%02x%02x%02x", input.r, input.g, input.b, __lux_tmp_11)
    end
    function vgui.safeRemove(panel)
      if valid.is(panel) then
        panel:Remove()
        return true
      end
      return false
    end
    function vgui.clearChildren(panel)
      if not valid.is(panel) then
        return 0
      end
      local children = panel:GetChildren()
      for index = 1, #children do
        local child = children[index]
        if valid.is(child) then
          child:Remove()
        end
      end
      return #children
    end
    function vgui.setTextIfChanged(panel, text)
      if not valid.is(panel) then
        return false
      end
      if panel:GetText() == text then
        return false
      end
      panel:SetText(text)
      return true
    end
  end
  
  __lux_exports.valid = valid
  __lux_exports.hookx = hookx
  __lux_exports.timerx = timerx
  __lux_exports.netx = netx
  __lux_exports.json = json
  __lux_exports.players = players
  __lux_exports.entsx = entsx
  __lux_exports.color = color
  __lux_exports.vgui = vgui
  
  return __lux_exports
end
