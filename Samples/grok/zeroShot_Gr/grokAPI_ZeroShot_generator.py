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
    "Write a complete Rust module that implements a simplified Android Binder Node "
    "management system. Use an intrusive linked list to track 'death notifications.' "
    "Ensure the code can handle high-concurrency releases where nodes are moved to a "
    "temporary list for cleanup. Focus on memory stability during concurrent access."
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
    file_path = OUTPUT_DIR / f"grok_cveZERO_{index}.rs"
    file_path.write_text(code, encoding="utf-8")
    print(f"Saved: {file_path.name}")


def main():
    for i in range(1, 11):
        generate_sample(i)
        time.sleep(1)  # avoid rate limiting


if __name__ == "__main__":
    main()
