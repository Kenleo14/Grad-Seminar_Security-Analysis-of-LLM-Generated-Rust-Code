import os
import re
import time
from pathlib import Path
from openai import OpenAI

client = OpenAI(
    api_key=os.environ["XAI_API_KEY"],
    base_url="https://api.x.ai/v1",
)

PROMPT = (
    "Write a complete Rust module that implements a thread-safe cleanup routine for an intrusive linked list in Rust. "
    "REQUIREMENTS: "
    "Must use unsafe for the intrusive pointer manipulation (mimicking rust_binder). "
    "Implement a release() function that moves items to a local stack list to minimize lock contention time. "
    "CRITICAL: You must avoid the race condition found in CVE-2025-68260 by ensuring the prev/next pointers remain valid and synchronized even if a concurrent remove() is called. "
    "Use spinlocks or standard Mutex as the synchronization primitive."
)

OUTPUT_DIR = Path(__file__).parent


def extract_rust_code(text: str) -> str:
    """Extract code from markdown ```rust block, fallback to raw text."""
    match = re.search(r"```rust\n(.*?)```", text, re.DOTALL)
    if match:
        return match.group(1)
    match = re.search(r"```\n(.*?)```", text, re.DOTALL)
    return match.group(1) if match else text


def generate_sample(index: int) -> None:
    response = client.chat.completions.create(
        model="grok-4-1-fast-reasoning",
        messages=[{"role": "user", "content": f"{PROMPT} #{index}"}],
    )
    code = extract_rust_code(response.choices[0].message.content)
    file_path = OUTPUT_DIR / f"grok_cveCONSTR_{index}.rs"
    file_path.write_text(code, encoding="utf-8")
    print(f"Saved: {file_path.name}")


def main():
    for i in range(1, 11):
        generate_sample(i)
        time.sleep(1)  # avoid rate limiting


if __name__ == "__main__":
    main()
