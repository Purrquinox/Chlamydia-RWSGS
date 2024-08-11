import requests
import os
import subprocess
import hashlib
import shutil
import time
from pathlib import Path
from dotenv import load_dotenv
from datetime import datetime
from alive_progress import alive_bar

# Load environment variables from .env file
load_dotenv()

# Variables
repo_name = "Purrquinox/Chlamydia-RWSGS"
bin_directory = "bin"
build_time = datetime.now().strftime('%Y-%m-%d | %H:%M:%S')
combos = [
    "linux/amd64",
    "linux/arm64",
    "windows/amd64",
    "windows/arm64"
]
comboCompilers = {
    "linux/amd64": "x86_64-unknown-linux-gnu",
    "linux/arm64": "x86_64-unknown-linux-gnu",
    "windows/amd64": "x86_64-pc-windows-gnu",
    "windows/arm64": "x86_64-pc-windows-gnu"
}

# Get GitHub token from environment variables
token = os.getenv("GITHUB_TOKEN")
if not token:
    print("GitHub token not found. Please set GITHUB_TOKEN in your .env file.")
    exit(1)

# Inputs
release_name = input("Release Name: ").strip()
release_body = input("Release Body: ").strip()
tag_name = input("Tag Name: ").strip()

# Check if the inputs were filled
if not release_name or not release_body or not tag_name:
    print("Please fill in all required inputs.")
    exit(1)

# Start compiling everything
print("Compiling for 'release' build.")
with alive_bar(len(combos)) as bar:
    for combo in combos:
        try:
            # Print
            print(f"Building for {combo}...")
            # Combo path
            combo_path = Path(f"{bin_directory}/{combo}")
            combo_path.mkdir(parents=True, exist_ok=True)
            # Variables
            compiler = comboCompilers[combo]
            # Render command(s)
            build_command = (
                f"cargo build --release -Z unstable-options --target {compiler} --out-dir {combo_path}"
            )
            # Create a subprocess for the command to be executed
            subprocess.run(build_command, shell=True, check=True)
            # Generate SHA256 checksum
            sha512 = hashlib.sha512()
            ppa = ''
            if combo == "windows/amd64":
                ppa = ".exe"
            elif combo == "windows/arm64":
                ppa = ".exe"
                
            with open(f"{combo_path}/rwsgs{ppa}", 'rb') as f:
                for chunk in iter(lambda: f.read(4096), b""):
                    sha512.update(chunk)
            sha512_checksum = sha512.hexdigest()
            with open(f"{combo_path}/checksum.sha512", 'w') as sha_file:
                sha_file.write(f"{sha512_checksum}  {combo_path}/checksum.sha512")
            bar()
        except subprocess.CalledProcessError as e:
            print(f"Error occurred while building for {combo}: {e}")
            exit(1)

time.sleep(2)
print("Compiled!")
time.sleep(1)
print("Uploading 'release' to GitHub!")

# Rename Linux executables
for folder in Path(f"{bin_directory}/linux").glob("*"):
    core_file = folder / "rwsgs"
    if core_file.exists():
        core_file.rename(core_file.with_suffix(".sh"))

# Create a release
release_url = f"https://api.github.com/repos/{repo_name}/releases"
headers = {
    "Authorization": f"Bearer {token}",
    "Accept": "application/vnd.github.v3+json",
    "X-GitHub-Api-Version": "2022-11-28"
}
data = {
    "tag_name": tag_name,
    "name": release_name,
    "body": release_body,
    "draft": False,
    "prerelease": False,
    "latest": True
}

# Send the request to create the release
response = requests.post(release_url, headers=headers, json=data)

# Check the response status code and handle errors
if response.status_code == 201:
    release = response.json()

    upload_url = release.get("upload_url", "").replace("{?name,label}", "")
    if not upload_url:
        print("Upload URL not found in the response.")
    else:
        # Recursively find and list all files in the bin directory
        file_paths = []
        for root, dirs, files in os.walk(bin_directory):
            for file in files:
                file_paths.append(os.path.join(root, file))

        if not file_paths:
            print(f"No files found in directory {bin_directory}.")
            exit(1)

        # Upload files to the release
        upload_headers = {
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/octet-stream",
        }

        with alive_bar(len(file_paths)) as bar:
            for file_path in file_paths:
                # Create a file name with subdirectory type and file name
                relative_path = os.path.relpath(file_path, bin_directory)
                dir_name, file_name = os.path.split(relative_path)
                new_file_name = f"{dir_name}-{file_name}"  # Include subdirectory in the file name

                with open(file_path, "rb") as file:
                    upload_response = requests.post(
                        f"{upload_url}?name={new_file_name}",
                        headers=upload_headers,
                        data=file
                    )
                # Check the response for each file upload
                if upload_response.status_code == 201:
                    bar()
                else:
                    print(f"Failed to upload {new_file_name}: {upload_response.status_code}")
                    print(upload_response.json())
else:
    print(f"Failed to create release: {response.status_code}")
    print(response.json())

# Cleanup
if os.path.exists(bin_directory):
    shutil.rmtree(bin_directory)
    shutil.rmtree("target")
    print("Finished!")
else:
    print(f"Directory {bin_directory} does not exist.")