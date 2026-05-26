class ToolRegistry {
  constructor() {
    this.tools = new Map();
    this.nextId = 1;
  }

  register(name, handler) {
    const id = `tool_${this.nextId++}`;
    this.tools.set(id, { id, name, handler });
    return id;
  }

  unregister(toolId) {
    this.tools.delete(toolId);
  }

  get(toolId) {
    return this.tools.get(toolId);
  }

  list() {
    return Array.from(this.tools.values()).map((t) => ({ id: t.id, name: t.name }));
  }
}

class GatewaySimulator {
  constructor(registry) {
    this.registry = registry;
    this.conversations = new Map();
  }

  createConversation(conversationId) {
    this.conversations.set(conversationId, []);
  }

  // 模拟一些网关/框架会做的“历史重放校验”：
  // 回放上下文时检查历史 tool_call 里的 tool_id 是否仍在当前注册表中。
  // 如果工具被注销，这里就会报错。
  validateContext(conversationId) {
    const history = this.conversations.get(conversationId) ?? [];
    for (const msg of history) {
      if (msg.type === 'tool_call') {
        const tool = this.registry.get(msg.toolId);
        if (!tool) {
          const error = new Error(
            `GatewayError: tool id not found: ${msg.toolId} (from historical call ${msg.callId})`,
          );
          error.code = 'TOOL_ID_NOT_FOUND';
          throw error;
        }
      }
    }
  }

  continueConversation(conversationId, userText) {
    this.validateContext(conversationId);
    const history = this.conversations.get(conversationId);
    history.push({ type: 'user', text: userText });
    return { ok: true, reply: `assistant: 收到 -> ${userText}` };
  }
}

class MiniAgent {
  constructor(registry, gateway, conversationId) {
    this.registry = registry;
    this.gateway = gateway;
    this.conversationId = conversationId;
  }

  // 简化：当用户提到 add(a,b) 就调用第一个名为 add 的工具
  runTurn(userText) {
    const history = this.gateway.conversations.get(this.conversationId);
    history.push({ type: 'user', text: userText });

    const match = userText.match(/add\((\d+),(\d+)\)/i);
    if (!match) {
      history.push({ type: 'assistant', text: 'assistant: no tool call' });
      return 'assistant: no tool call';
    }

    const a = Number(match[1]);
    const b = Number(match[2]);

    const addTool = this.registry.list().find((t) => t.name === 'add');
    if (!addTool) {
      history.push({ type: 'assistant', text: 'assistant: add tool missing' });
      return 'assistant: add tool missing';
    }

    const tool = this.registry.get(addTool.id);
    const callId = `call_${Date.now()}`;

    history.push({
      type: 'tool_call',
      callId,
      toolId: tool.id,
      toolName: tool.name,
      args: { a, b },
    });

    const result = tool.handler({ a, b });

    history.push({
      type: 'tool_result',
      callId,
      toolId: tool.id,
      output: result,
    });

    const reply = `assistant: result = ${result}`;
    history.push({ type: 'assistant', text: reply });
    return reply;
  }
}

function runExperiment() {
  const registry = new ToolRegistry();
  const gateway = new GatewaySimulator(registry);
  const conversationId = 'conv_1';
  gateway.createConversation(conversationId);

  const addToolId = registry.register('add', ({ a, b }) => a + b);
  console.log('[1] register tool:', addToolId, registry.list());

  const agent = new MiniAgent(registry, gateway, conversationId);
  const r1 = agent.runTurn('please do add(2,3)');
  console.log('[2] first turn:', r1);

  registry.unregister(addToolId);
  console.log('[3] unregister tool:', addToolId, registry.list());

  try {
    const r2 = gateway.continueConversation(conversationId, '继续聊，不调用工具');
    console.log('[4] second turn:', r2);
  } catch (err) {
    console.error('[4] second turn failed:');
    console.error('    code   =', err.code);
    console.error('    detail =', err.message);
  }

  console.log('\n--- conversation history ---');
  const history = gateway.conversations.get(conversationId);
  for (const [i, item] of history.entries()) {
    console.log(String(i + 1).padStart(2, '0'), JSON.stringify(item));
  }
}

runExperiment();
