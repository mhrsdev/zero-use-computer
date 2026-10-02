# The model sees exactly this in both arms.
SYSTEM_PROMPT='You control a Linux desktop through the computer-use tools (mcp__cu__*). They are your only tools. Work carefully and efficiently. When the task is complete, reply with the single word DONE. If you cannot complete it, reply with FAILED: followed by the reason.'
TASK_PROMPT='The diagram editor Dia is open with the file services.dia. Delete the box labelled exactly "Cache" (not "Cache Monitor", not any other box) and leave everything else in the diagram unchanged. Then save the file in place with Ctrl+S.'
MODEL='claude-sonnet-5-5'
EFFORT='medium'
MAX_TURNS=40
RUN_TIMEOUT=900   # seconds for the whole agent run
