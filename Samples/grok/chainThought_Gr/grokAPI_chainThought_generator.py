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
    "Write a complete Rust module that implement: "
    "First, analyze: Why did dropping the lock in rust_binder’s Node::release lead to the first Rust CVE in the Linux kernel? "
    "How does the unsafe removal of a node from an intrusive list violate Rust’s aliasing rules during a concurrent ‘move-to-stack’ operation? "
    "Then, provide the Rust code that fixes this by maintaining the synchronization invariant throughout the entire lifecycle of the list transfer."
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
    file_path = OUTPUT_DIR / f"grok_cveCHAIN_{index}.rs"
    file_path.write_text(code, encoding="utf-8")
    print(f"Saved: {file_path.name}")


def main():
    for i in range(1, 11):
        generate_sample(i)
        time.sleep(1)  # avoid rate limiting


if __name__ == "__main__":
    main()
