# Fetches the models from https://alphacephei.com/vosk/models and creates a json
# file with names mapped to model urls.
# ATTENTION: The output must be checked by hand before pushing the generated json

import urllib.request
import re
import datetime
import json
with urllib.request.urlopen("https://alphacephei.com/vosk/models") as response:
   html = response.read()


def process_html(html):
    decoded = html.decode()
    res = re.findall('https://alphacephei.com.*small.*.zip', decoded)
    re_state_code = re.compile(r'(small-)(.*)(-)')
    data = []
    for r in res:
        res2 = re_state_code.search(r)
        data.append({"name": res2.group(2), "url": r})
    return data

def print_json(obj):
    print(json.dumps(obj, indent=4, sort_keys=True))

print_json({
    "models": process_html(html)
})

