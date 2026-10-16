from . import pricing


def render(items, tax_rate=0.0):
    lines = [f"{qty} x {price:.2f}" for price, qty in items]
    lines.append(f"total {pricing.calc_total(items, tax_rate):.2f}")
    return "\n".join(lines)
