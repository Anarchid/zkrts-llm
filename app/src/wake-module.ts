/**
 * WakeModule — Agent-controlled event filtering.
 *
 * Lets the agent specify which SAI events should wake it up.
 * Events that don't match are still delivered to context but
 * don't trigger inference.
 *
 * Conditions persist until the agent calls set_conditions again.
 * The agent should call set_conditions at the end of every
 * think cycle to set what should wake it next.
 *
 * Pending wake: if a matching event arrives while inference is
 * already running (debounce window), we track it. When inference
 * completes and the agent calls set_conditions, we fire a new
 * inference immediately if there's a pending match. This also
 * covers trace-based detection: onProcess sees inference-request
 * events and can check pending state.
 */

import type {
  Module,
  ModuleContext,
  ProcessState,
  ProcessEvent,
  EventResponse,
  ToolDefinition,
  ToolCall,
  ToolResult,
} from '@connectome/agent-framework';

interface WakeConditions {
  events: string[];
  timeout_s: number;
}

export class WakeModule implements Module {
  readonly name = 'wake';
  private ctx: ModuleContext | null = null;
  private conditions: WakeConditions | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lastTriggerTime = 0;
  private pendingWake = false;
  private static DEBOUNCE_MS = 500;

  /**
   * Callback for MCPLModule's shouldTriggerInference.
   * Arrow function to preserve `this` binding.
   */
  shouldTrigger = (content: string, _metadata: Record<string, unknown>): boolean => {
    // No conditions set (initial state) — trigger on everything
    if (!this.conditions) return true;

    // Try to extract SAI event type from the message content.
    let matches = false;
    try {
      const jsonMatch = content.match(/\{[^]*\}/);
      if (jsonMatch) {
        const parsed = JSON.parse(jsonMatch[0]);
        if (parsed.type && this.conditions.events.includes(parsed.type)) {
          matches = true;
        }
      }
    } catch {
      // Not JSON — don't trigger
    }

    if (!matches) return false;

    // Debounce: collapse same-frame events into one inference
    const now = Date.now();
    if (now - this.lastTriggerTime < WakeModule.DEBOUNCE_MS) {
      // Event matches but we're in debounce window (likely mid-inference).
      // Mark as pending — will fire after current inference completes.
      this.pendingWake = true;
      return false;
    }

    this.lastTriggerTime = now;
    this.pendingWake = false;
    return true;
  };

  async start(ctx: ModuleContext): Promise<void> {
    this.ctx = ctx;
  }

  async stop(): Promise<void> {
    this.clearTimer();
    this.ctx = null;
  }

  getTools(): ToolDefinition[] {
    return [
      {
        name: 'set_conditions',
        description:
          'Set wake conditions. You will sleep until a matching event arrives or the timeout ' +
          'expires. All events are still recorded — you will see them when you wake up. ' +
          'Conditions persist until you call set_conditions again.',
        inputSchema: {
          type: 'object' as const,
          properties: {
            events: {
              type: 'array',
              items: { type: 'string' },
              description:
                'SAI event types to wake on (e.g. unit_finished, enemy_enter_los, unit_damaged, unit_idle)',
            },
            timeout_s: {
              type: 'number',
              description: 'Maximum sleep duration in seconds (default: 30)',
            },
          },
          required: ['events'],
        },
      },
    ];
  }

  async handleToolCall(call: ToolCall): Promise<ToolResult> {
    const input = call.input as { events: string[]; timeout_s?: number };

    if (!input.events || !Array.isArray(input.events) || input.events.length === 0) {
      return { success: false, error: 'events must be a non-empty array of event type strings' };
    }

    const timeout_s = input.timeout_s ?? 30;

    // Clear previous timer
    this.clearTimer();

    // Check if a matching event arrived during the last inference cycle.
    // If so, fire inference immediately with the accumulated events.
    const hadPendingWake = this.pendingWake;
    this.pendingWake = false;

    // Set new conditions
    this.conditions = { events: input.events, timeout_s };

    if (hadPendingWake) {
      // Schedule immediate wake — use setTimeout(0) so the tool result
      // is delivered first, then inference fires with all accumulated events.
      setTimeout(() => {
        this.ctx?.pushEvent({
          type: 'inference-request',
          agentName: 'commander',
          reason: 'pending-wake',
          source: 'wake',
          triggerInference: true,
        } as any);
      }, 0);

      return {
        success: true,
        data: `Waking immediately — matching event(s) arrived during last inference. Will then sleep on: [${input.events.join(', ')}] or after ${timeout_s}s.`,
        endTurn: true,
      };
    }

    // Start timeout
    this.timer = setTimeout(() => {
      this.timer = null;
      this.ctx?.pushEvent({
        type: 'inference-request',
        agentName: 'commander',
        reason: 'wake-timeout',
        source: 'wake',
        triggerInference: true,
      } as any);
    }, timeout_s * 1000);

    return {
      success: true,
      data: `Sleeping. Will wake on: [${input.events.join(', ')}] or after ${timeout_s}s.`,
      endTurn: true,
    };
  }

  async onProcess(event: ProcessEvent, _state: ProcessState): Promise<EventResponse> {
    // Handle system bootstrap messages
    if (event.type === 'external-message' && event.source === 'system') {
      return {
        addMessages: [{
          participant: 'user',
          content: [{ type: 'text', text: String(event.content) }],
        }],
        requestInference: true,
      };
    }
    return {};
  }

  private clearTimer(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
