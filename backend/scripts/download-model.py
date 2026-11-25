"""Resumable, bounded parallel GGUF download with pinned SHA-256 verification."""
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import os
import shutil
from pathlib import Path
import sys
import threading
import time
import urllib.request
sys.path.insert(0, str(Path(__file__).resolve().parent))
import runtime

def digest(path, notify=lambda *_: None):
    total = path.stat().st_size
    done, last, value = 0, 0, hashlib.sha256()
    with path.open('rb') as source:
        while block := source.read(4 * 1024 * 1024):
            value.update(block)
            done += len(block)
            if time.monotonic() - last > 0.5:
                notify('Проверка SHA-256 модели', done / max(total, 1))
                last = time.monotonic()
    return value.hexdigest()

def download(notify=lambda *_: None):
    model = runtime.read(runtime.ROOT / 'config/local-model.json')
    target = runtime.model_path()
    stamp_path = runtime.HOME / 'model-verification.json'
    def stamp():
        stat = target.stat()
        runtime.save(stamp_path, {'path': str(target), 'size': stat.st_size, 'mtime': stat.st_mtime_ns, 'sha256': model['sha256']})
    if target.exists() and target.stat().st_size == model['sizeBytes']:
        stat, previous = target.stat(), runtime.read(stamp_path)
        if previous == {'path': str(target), 'size': stat.st_size, 'mtime': stat.st_mtime_ns, 'sha256': model['sha256']}:
            notify('Используется установленная модель', 1)
            return target
        if digest(target, notify) == model['sha256']:
            stamp()
            return target
        raise RuntimeError('У существующего GGUF другая контрольная сумма. Исходный файл сохранён; проверьте модель в LM Studio.')
    target.parent.mkdir(parents=True, exist_ok=True)
    pieces = runtime.HOME / 'downloads' / model['sha256']
    pieces.mkdir(parents=True, exist_ok=True)
    size, chunk = model['sizeBytes'], 64 * 1024 * 1024
    remaining = max(0, size - sum(path.stat().st_size for path in pieces.glob('*.part')))
    same_volume = pieces.stat().st_dev == target.parent.stat().st_dev
    target_required = size + (remaining if same_volume else 0)
    if shutil.disk_usage(target.parent).free < target_required or (not same_volume and shutil.disk_usage(pieces).free < remaining):
        raise RuntimeError(f'Недостаточно места для модели. Нужно до {target_required / 1e9:.1f} GB свободного места с учётом временных частей.')
    url = f'https://huggingface.co/{model["repository"]}/resolve/{model["revision"]}/{model["filename"]}'
    ranges = [(i, start, min(start + chunk, size)-1) for i, start in enumerate(range(0, size, chunk))]
    complete, guard = 0, threading.Lock()
    def fetch(item):
        nonlocal complete
        index, start, end = item
        path = pieces / f'{index:05}.part'
        expected = end - start + 1
        downloaded = path.stat().st_size if path.exists() else 0
        if downloaded > expected:
            path.write_bytes(b'')
            downloaded = 0
        with guard:
            complete += downloaded
        for attempt in range(4):
            try:
                if downloaded < expected:
                    request = urllib.request.Request(url, headers={'Range': f'bytes={start + downloaded}-{end}', 'User-Agent':'SupaDev/0.1'})
                    with urllib.request.urlopen(request, timeout=60) as response:
                        if response.status != 206 or not response.headers.get('Content-Range','').startswith(f'bytes {start+downloaded}-{end}/'):
                            raise RuntimeError('Сервер не подтвердил диапазон загрузки.')
                        with path.open('ab') as out:
                            while block := response.read(min(1024 * 1024, expected-downloaded)):
                                out.write(block)
                                downloaded += len(block)
                                with guard:
                                    complete += len(block)
                                if downloaded == expected:
                                    break
                if downloaded != expected:
                    raise RuntimeError('Неполная часть модели.')
                return path
            except Exception:
                if attempt == 3:
                    raise
                time.sleep(1 + attempt)
    with ThreadPoolExecutor(max_workers=8) as pool:
        futures = [pool.submit(fetch, item) for item in ranges]
        while not all(future.done() for future in futures):
            notify(f'Загрузка модели: {complete / 1e9:.1f} / {size / 1e9:.1f} GB', complete / size)
            time.sleep(0.5)
        for future in as_completed(futures):
            future.result()
    staged = target.with_name(target.name + '.download')
    value, copied = hashlib.sha256(), 0
    with staged.open('wb') as out:
        for index, _, _ in ranges:
            with (pieces / f'{index:05}.part').open('rb') as source:
                while block := source.read(4 * 1024 * 1024):
                    out.write(block)
                    value.update(block)
                    copied += len(block)
            notify('Сборка и проверка модели', copied / size)
    if value.hexdigest() != model['sha256']:
        for path in pieces.glob('*.part'):
            assert path.resolve().is_relative_to((runtime.HOME / 'downloads').resolve())
            path.unlink()
        staged.unlink()
        raise RuntimeError('SHA-256 загруженной модели не совпал. Повреждённые временные части удалены; повторите загрузку.')
    os.replace(staged, target)
    stamp()
    for path in pieces.glob('*.part'):
        assert path.resolve().is_relative_to((runtime.HOME / 'downloads').resolve())
        path.unlink()
    notify('Модель скачана и проверена', 1)
    return target
