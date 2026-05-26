class ToolRegistry {
  constructor() {
    this.active = new Map();
    this.archived = new Map();
    this.nextId = 1;
  }

  register(name, handler) {
    const id = `tool_${this.nextId++}`;
    this.active.set(id, { id, name, handler });
    return id;
  }

  unregister(toolId) {
    const tool = this.active.get(toolId);
    if (!tool) return;
    this.active.delete(toolId);
    // 关键点：注销后不丢历史定义，归档为 tombstone/snapshot
    this.archived.set(toolId, { id: tool.id, name: tool.name });
  }

  resolveForHistory(toolId) {
    return this.active.get(toolId) || this.archived.get(toolId) || null;
  }

  getActiveByName(name) {
    return Array.from(this.active.values()).find((t) => t.name === name);
  }

  getActive(toolId) {
    return this.active.get(toolId);
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

  validateContext(conversationId) {
    const history = this.conversations.get(conversationId) ?? [];
    for (const msg of history) {
      if (msg.type === 'tool_call') {
        const tool = this.registry.resolveForHistory(msg.toolId);
        if (!tool) {
          throw new Error(`GatewayError: unknown historical tool_id=${msg.toolId}`);
        }
      }
    }
  }

  continueConversation(conversationId, userText) {
    this.validateContext(conversationId);
    const history = this.conversations.get(conversationId);
    history.push({ type: 'user', text: userText });
    history.push({ type: 'assistant', text: 'assistant: 上下文继续成功（即使工具已注销）' });
    return { ok: true };
  }
}

class MiniAgent {
  constructor(registry, gateway, conversationId) {
    this.registry = registry;
    this.gateway = gateway;
    this.conversationId = conversationId;
  }

  runTurn(userText) {
    const history = this.gateway.conversations.get(this.conversationId);
    history.push({ type: 'user', text: userText });

    const match = userText.match(/add\((\d+),(\d+)\)/i);
    if (!match) {
      history.push({ type: 'assistant', text: 'assistant: no tool call' });
      return;
    }

    const toolMeta = this.registry.getActiveByName('add');
    if (!toolMeta) {
      history.push({ type: 'assistant', text: 'assistant: add tool missing' });
      return;
    }

    const tool = this.registry.getActive(toolMeta.id);
    const a = Number(match[1]);
    const b = Number(match[2]);
    const callId = `call_${Date.now()}`;

    history.push({ type: 'tool_call', callId, toolId: tool.id, args: { a, b } });
    const output = tool.handler({ a, b });
    history.push({ type: 'tool_result', callId, toolId: tool.id, output });
    history.push({ type: 'assistant', text: `assistant: result=${output}` });
  }
}

function run() {
  const registry = new ToolRegistry();
  const gateway = new GatewaySimulator(registry);
  const convId = 'conv_safe';
  gateway.createConversation(convId);

  const toolId = registry.register('add', ({ a, b }) => a + b);
  const agent = new MiniAgent(registry, gateway, convId);

  agent.runTurn('add(7,8)');
  registry.unregister(toolId);

  // 即使注销，历史校验仍通过
  gateway.continueConversation(convId, '继续聊');

  console.log('PASS: context continued after tool unregister.');
  console.log(JSON.stringify(gateway.conversations.get(convId), null, 2));
}

run();
