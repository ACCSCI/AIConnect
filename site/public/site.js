const command = document.getElementById('install-command');
const copy = document.getElementById('copy-command');
const status = document.getElementById('copy-status');
// The current origin is only a distribution address; it is never an API target.
command.textContent = `npx skills add ${window.location.origin} -g -a codex -y`;
copy.addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(command.textContent);
    status.textContent = '已复制 Skill 安装命令。安装完成后，让 Agent 初始化 AIConnect。';
  } catch {
    status.textContent = '浏览器未允许复制。请选中上方命令手动复制。';
  }
});
