return function(__lux_import)
  local __lux_exports = {}
  local activeEffect
  local effectStack
  local batchDepth
  local pendingEffects
  local cleanup
  local track
  local runEffect
  local flush
  local schedule
  local notify
  local signal
  local effect
  local memo
  local batch
  local untrack
  local peek
  do
    activeEffect = nil
    effectStack = {}
    batchDepth = 0
    pendingEffects = {}
    cleanup = function(effectState)
      for dep in pairs(effectState.deps) do
        dep.subscribers[effectState] = nil
      end
      effectState.deps = {}
    end
    track = function(dep)
      if activeEffect == nil then
        return nil
      end
      dep.subscribers[activeEffect] = true
      activeEffect.deps[dep] = true
    end
    runEffect = function(effectState)
      if effectState.disposed then
        return nil
      end
      cleanup(effectState)
      effectStack[#effectStack + 1] = effectState
      activeEffect = effectState
      local ok, result = pcall(effectState.run)
      effectStack[#effectStack] = nil
      activeEffect = effectStack[#effectStack]
      if not ok then
        error(result)
      end
      return result
    end
    flush = function()
      local queued = pendingEffects
      pendingEffects = {}
      for effectState in pairs(queued) do
        runEffect(effectState)
      end
    end
    schedule = function(effectState)
      if effectState.disposed then
        return nil
      end
      if batchDepth > 0 then
        pendingEffects[effectState] = true
        return nil
      end
      return runEffect(effectState)
    end
    notify = function(dep)
      for effectState in pairs(dep.subscribers) do
        schedule(effectState)
      end
    end
    signal = function(initial)
      local dep = { value = initial, subscribers = {} }
      return function(...)
        local __lux_cmp_1 = false
        if select("#", ...) ~= nil and 0 ~= nil then
          __lux_cmp_1 = select("#", ...) > 0
        end
        if __lux_cmp_1 then
          local nextValue = select(1, ...)
          if dep.value ~= nextValue then
            dep.value = nextValue
            notify(dep)
          end
          return dep.value
        end
        track(dep)
        return dep.value
      end
    end
    effect = function(callback)
      local effectState = { run = callback, deps = {}, disposed = false }
      runEffect(effectState)
      return function()
        if effectState.disposed then
          return nil
        end
        effectState.disposed = true
        return cleanup(effectState)
      end
    end
    memo = function(callback)
      local value = signal(nil)
      effect(
        function()
          return value(callback())
        end
      )
      return function()
        return value()
      end
    end
    batch = function(callback)
      batchDepth = batchDepth + 1
      local ok, result = pcall(callback)
      batchDepth = batchDepth - 1
      if batchDepth == 0 then
        flush()
      end
      if not ok then
        error(result)
      end
      return result
    end
    untrack = function(callback)
      local previous = activeEffect
      activeEffect = nil
      local ok, result = pcall(callback)
      activeEffect = previous
      if not ok then
        error(result)
      end
      return result
    end
    peek = function(readable)
      return untrack(
        function()
          return readable()
        end
      )
    end
  end
  
  __lux_exports.signal = signal
  __lux_exports.effect = effect
  __lux_exports.memo = memo
  __lux_exports.batch = batch
  __lux_exports.untrack = untrack
  __lux_exports.peek = peek
  
  return __lux_exports
end
