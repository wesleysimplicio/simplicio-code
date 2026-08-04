from pathlib import Path


ROOT = Path(__file__).parents[2]
ONBOARDING_DOCS = (
    ROOT / "README.md",
    ROOT / "docs" / "QUICKSTART.md",
    ROOT / "docs" / "TROUBLESHOOTING.md",
)


def test_product_onboarding_never_requests_upstream_provider_credentials():
    for path in ONBOARDING_DOCS:
        text = path.read_text(encoding="utf-8").lower()
        assert "openrouter_api_key" not in text
        assert "openrouter" not in text
        assert "api.openai.com" not in text
