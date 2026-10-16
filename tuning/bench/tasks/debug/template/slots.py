def merge(busy):
    merged = []
    for start, end in busy:
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], end)
        else:
            merged.append((start, end))
    return merged


def free_slots(busy, day_start, day_end, min_len):
    slots = []
    cursor = day_start
    for start, end in merge(busy):
        if start - cursor > min_len:
            slots.append((cursor, start))
        cursor = end
    if day_end - cursor > min_len:
        slots.append((cursor, day_end))
    return slots
