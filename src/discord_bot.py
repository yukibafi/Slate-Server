import asyncio
import hashlib
import json
import os
import random
import re
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from typing import Optional

import discord
from discord import app_commands
from dotenv import load_dotenv

load_dotenv()
TOKEN = os.getenv("DISCORD_TOKEN")
# Optional: extra people (besides server admins) allowed to DM the bot a server IP.
# Put their Discord user IDs in .env as ALLOWED_USER_IDS=123,456
ALLOWED_IDS = {int(x) for x in os.getenv("ALLOWED_USER_IDS", "").replace(" ", "").split(",") if x.isdigit()}

# ---------------------------------------------------------------
# COLOURS (change these to restyle every embed the bot sends)
# ---------------------------------------------------------------
PINK = discord.Colour.from_str("#FF8FCB")
ORANGE = discord.Colour.from_str("#F99E1A")   # Overwatch
GREEN = discord.Colour.from_str("#5FBF5B")    # Minecraft
PURPLE = discord.Colour.from_str("#A78BFA")

# ---------------------------------------------------------------
# ROLES: name -> (colour, button emoji, button label)
# ---------------------------------------------------------------
MC_ROLE = "🌲 Minecraft"
OW_ROLE = "🧡 Overwatch"
ROLES = {
    MC_ROLE: (GREEN, "🌲", "Minecraft"),
    OW_ROLE: (ORANGE, "🧡", "Overwatch"),
}

# ---------------------------------------------------------------
# EDIT THIS to change your server. /setup creates anything missing
# and skips anything that already exists, so it's safe to re-run.
#
#   "role": MC_ROLE   -> only people with that role can see the section
#   no "role"         -> everyone can see it
#   "readonly": True  -> only the bot can post in its text channels
# ---------------------------------------------------------------
ROLE_CHANNEL = "👋┃ᴘɪᴄᴋ-ʏᴏᴜʀ-ʀᴏʟᴇꜱ"  # the role buttons get posted here

LAYOUT = {
    "✦ ━━ ꜱᴛᴀʀᴛ ʜᴇʀᴇ ━━ ✦": {
        "readonly": True,
        "text": [ROLE_CHANNEL],
    },
    "✦ ━━ ᴛʜᴇ ʟᴏᴜɴɢᴇ ━━ ✦": {
        "text": ["💬┃ᴄʜɪᴛᴄʜᴀᴛ", "😹┃ᴍᴇᴍᴇꜱ"],
        "voice": ["🔊┃ʜᴀɴɢᴏᴜᴛ"],
    },
    "✦ ━━ 🌲 ᴍɪɴᴇᴄʀᴀꜰᴛ ━━ ✦": {
        "role": MC_ROLE,
        "text": ["🌲┃ᴍᴄ-ᴄʜᴀᴛ"],
        "voice": ["🔊┃ᴍɪɴᴇᴄʀᴀꜰᴛ"],
    },
    "✦ ━━ 🧡 ᴏᴠᴇʀᴡᴀᴛᴄʜ ━━ ✦": {
        "role": OW_ROLE,
        "text": ["🧡┃ᴏᴡ-ᴄʜᴀᴛ"],
        "voice": ["🔊┃ᴏᴠᴇʀᴡᴀᴛᴄʜ"],
    },
    "✦ ━━ ᴀꜰᴋ ━━ ✦": {
        "voice": ["💤┃ᴀꜰᴋ"],
    },
}

EIGHT_BALL = [
    "yes!! 💖", "definitely ✨", "100% yes", "ask again later 🔮",
    "hmm, not sure 🤔", "don't count on it", "nope 😔", "very doubtful",
]
HUGS = ["🤗", "💞", "🫂", "✨", "💖"]


def fancy(title: str, description: str = "", colour: discord.Colour = PINK) -> discord.Embed:
    return discord.Embed(title=title, description=description, colour=colour)


# ------------------------- role buttons -------------------------

class RoleButton(discord.ui.Button):
    def __init__(self, role_name: str, emoji: str, label: str):
        super().__init__(
            label=label,
            emoji=emoji,
            style=discord.ButtonStyle.secondary,
            custom_id=f"role:{role_name}",
        )
        self.role_name = role_name

    async def callback(self, interaction: discord.Interaction):
        role = discord.utils.get(interaction.guild.roles, name=self.role_name)
        if role is None:
            await interaction.response.send_message(
                "That role doesn't exist yet. Ask an admin to run /setup.", ephemeral=True
            )
            return
        member = interaction.user
        if role in member.roles:
            await member.remove_roles(role)
            e = fancy("Removed 👋", f"**{self.label}** channels are hidden again.", PURPLE)
        else:
            await member.add_roles(role)
            e = fancy("Unlocked ✨", f"You now have **{self.label}**! New channels are showing.", role.colour)
        await interaction.response.send_message(embed=e, ephemeral=True)


class RoleView(discord.ui.View):
    def __init__(self):
        super().__init__(timeout=None)  # keeps the buttons working after restarts
        for name, (_, emoji, label) in ROLES.items():
            self.add_item(RoleButton(name, emoji, label))


def role_menu_embed() -> discord.Embed:
    e = fancy(
        "✦ ᴘɪᴄᴋ ᴡʜᴀᴛ ʏᴏᴜ ᴘʟᴀʏ ✦",
        "Click a button to unlock those channels.\nClick again to hide them.\nPick one, or both! 💖\n\n"
        "🌲 **Minecraft**: server chat and voice\n"
        "🧡 **Overwatch**: team chat and voice",
        PINK,
    )
    e.set_footer(text="Buttons toggle on and off")
    return e


class Bot(discord.Client):
    def __init__(self):
        super().__init__(
            intents=discord.Intents.default(),
            activity=discord.Game("with your server 💖"),
        )
        self.tree = app_commands.CommandTree(self)

    async def setup_hook(self):
        self.add_view(RoleView())
        self.add_view(WelcomeView())
        asyncio.create_task(modlist_loop())

    async def sync_guild(self, guild: discord.Guild):
        # Per-server sync shows up instantly (global sync can take an hour)
        self.tree.copy_global_to(guild=guild)
        await self.tree.sync(guild=guild)

    async def on_ready(self):
        for guild in self.guilds:
            await self.sync_guild(guild)
        print(f"Logged in as {self.user}. Bot is online!")

    async def on_guild_join(self, guild: discord.Guild):
        await self.sync_guild(guild)


bot = Bot()


# ------------------------- setup -------------------------

def make_overwrites(guild: discord.Guild, role: Optional[discord.Role], readonly: bool):
    """Hide from everyone except `role`, and/or make read-only. Bot keeps full access."""
    everyone = {}
    if role:
        everyone["view_channel"] = False
    if readonly:
        everyone["send_messages"] = False
        everyone["add_reactions"] = False
    if not everyone:
        return {}
    ow = {
        guild.default_role: discord.PermissionOverwrite(**everyone),
        guild.me: discord.PermissionOverwrite(
            view_channel=True, send_messages=True, manage_channels=True,
            read_message_history=True, embed_links=True,
        ),
    }
    if role:
        ow[role] = discord.PermissionOverwrite(view_channel=True)
    return ow


@bot.tree.command(name="setup", description="Create the roles, channels and role buttons for this server")
@app_commands.default_permissions(administrator=True)
@app_commands.guild_only()
async def setup(interaction: discord.Interaction):
    await interaction.response.defer(ephemeral=True)
    guild = interaction.guild
    made = []

    # 1. roles
    for role_name, (colour, _, _) in ROLES.items():
        if not discord.utils.get(guild.roles, name=role_name):
            await guild.create_role(name=role_name, colour=colour, mentionable=True)
            made.append(f"role {role_name}")

    # 2. categories + channels
    for cat_name, conf in LAYOUT.items():
        role = discord.utils.get(guild.roles, name=conf["role"]) if conf.get("role") else None
        readonly = conf.get("readonly", False)
        text_ow = make_overwrites(guild, role, readonly)
        voice_ow = make_overwrites(guild, role, False)

        category = discord.utils.get(guild.categories, name=cat_name)
        if category is None:
            category = await guild.create_category(cat_name, overwrites=text_ow)
            made.append(f"category {cat_name}")
        elif text_ow:
            await category.edit(overwrites=text_ow)  # keep visibility rules up to date

        for name in conf.get("text", []):
            if not discord.utils.get(category.text_channels, name=name):
                await guild.create_text_channel(name, category=category, overwrites=text_ow)
                made.append(f"#{name}")
        for name in conf.get("voice", []):
            if not discord.utils.get(category.voice_channels, name=name):
                await guild.create_voice_channel(name, category=category, overwrites=voice_ow)
                made.append(f"🔊 {name}")

    # 3. role buttons (posted once)
    ch = discord.utils.get(guild.text_channels, name=ROLE_CHANNEL)
    if ch:
        already = False
        try:
            async for m in ch.history(limit=20):
                if m.author == guild.me:
                    already = True
                    break
        except discord.HTTPException:
            pass
        if not already:
            await ch.send(embed=role_menu_embed(), view=RoleView())
            made.append("role buttons")

    if made:
        e = fancy("Server ready ✨", "\n".join(made), PINK)
    else:
        e = fancy("All good 💖", "Everything already exists, nothing to do.", PINK)
    await interaction.followup.send(embed=e, ephemeral=True)


@bot.tree.command(name="rolemenu", description="Post another copy of the role buttons in this channel")
@app_commands.default_permissions(administrator=True)
@app_commands.guild_only()
async def rolemenu(interaction: discord.Interaction):
    await interaction.channel.send(embed=role_menu_embed(), view=RoleView())
    await interaction.response.send_message("Posted!", ephemeral=True)


# ------------------------- fun commands -------------------------

@bot.tree.command(name="roll", description="Roll a dice")
@app_commands.describe(sides="How many sides (default 6)")
async def roll(interaction: discord.Interaction, sides: app_commands.Range[int, 2, 1000] = 6):
    e = fancy("🎲 Dice roll", f"# {random.randint(1, sides)}", PURPLE)
    e.set_footer(text=f"d{sides} for {interaction.user.display_name}")
    await interaction.response.send_message(embed=e)


@bot.tree.command(name="coinflip", description="Flip a coin")
async def coinflip(interaction: discord.Interaction):
    await interaction.response.send_message(embed=fancy("🪙 Coin flip", f"# {random.choice(['Heads', 'Tails'])}", PURPLE))


@bot.tree.command(name="8ball", description="Ask the magic 8-ball")
@app_commands.describe(question="Your question")
async def eight_ball(interaction: discord.Interaction, question: str):
    e = fancy("🔮 Magic 8-ball", f"*{question}*\n\n# {random.choice(EIGHT_BALL)}", PURPLE)
    await interaction.response.send_message(embed=e)


@bot.tree.command(name="choose", description="Can't decide? Let me pick")
@app_commands.describe(options="Separate choices with commas, e.g. pizza, sushi, tacos")
async def choose(interaction: discord.Interaction, options: str):
    picks = [o.strip() for o in options.split(",") if o.strip()]
    if len(picks) < 2:
        await interaction.response.send_message("Give me at least 2 options, separated by commas!", ephemeral=True)
        return
    await interaction.response.send_message(embed=fancy("✨ I pick", f"# {random.choice(picks)}", PINK))


@bot.tree.command(name="hug", description="Give someone a hug")
async def hug(interaction: discord.Interaction, user: discord.Member):
    e = fancy(f"{random.choice(HUGS)} Hug!", f"{interaction.user.mention} hugs {user.mention}", PINK)
    await interaction.response.send_message(embed=e)


@bot.tree.command(name="pat", description="Pat someone on the head")
async def pat(interaction: discord.Interaction, user: discord.Member):
    e = fancy("🐾 Pat pat", f"{interaction.user.mention} pats {user.mention}", PINK)
    await interaction.response.send_message(embed=e)


@bot.tree.command(name="hero", description="Get a random Overwatch hero to play")
@app_commands.describe(role="Pick a role, or leave blank for any")
@app_commands.choices(role=[
    app_commands.Choice(name="Tank", value="tank"),
    app_commands.Choice(name="DPS", value="dps"),
    app_commands.Choice(name="Support", value="support"),
])
async def hero(interaction: discord.Interaction, role: Optional[app_commands.Choice[str]] = None):
    heroes = {
        "tank": ["D.Va", "Reinhardt", "Winston", "Zarya", "Sigma", "Orisa", "Roadhog", "Junker Queen", "Mauga", "Ramattra", "Doomfist", "Wrecking Ball", "Hazard"],
        "dps": ["Tracer", "Genji", "Soldier: 76", "Reaper", "Cassidy", "Pharah", "Sojourn", "Widowmaker", "Hanzo", "Junkrat", "Mei", "Bastion", "Sombra", "Echo", "Symmetra", "Torbjörn", "Ashe", "Venture", "Freja"],
        "support": ["Mercy", "Ana", "Lúcio", "Zenyatta", "Moira", "Baptiste", "Brigitte", "Kiriko", "Lifeweaver", "Illari", "Juno", "Mizuki"],
    }
    icons = {"tank": "🛡️", "dps": "💥", "support": "💖"}
    key = role.value if role else random.choice(list(heroes))
    e = fancy("🧡 Your hero", f"# {random.choice(heroes[key])}\n{icons[key]} {key.upper()}", ORANGE)
    await interaction.response.send_message(embed=e)


@bot.tree.command(name="ping", description="Check the bot is awake")
async def ping(interaction: discord.Interaction):
    await interaction.response.send_message(embed=fancy("🏓 pong!", f"{round(bot.latency * 1000)}ms", PINK))


# ------------------------- DM the bot a server IP -------------------------

class IPChannelSelect(discord.ui.Select):
    def __init__(self, guild: discord.Guild, ip: str):
        options = [
            discord.SelectOption(
                label=f"#{c.name}"[:100],
                description=c.category.name[:100] if c.category else None,
                value=str(c.id),
            )
            for c in guild.text_channels[:25]
        ]
        super().__init__(placeholder="Which channel should I post it in?", options=options)
        self.guild = guild
        self.ip = ip

    async def callback(self, interaction: discord.Interaction):
        channel = self.guild.get_channel(int(self.values[0]))
        e = fancy("🌲 Server IP updated", f"```{self.ip}```", GREEN)
        e.timestamp = datetime.now(timezone.utc)
        await channel.send(embed=e)
        try:
            await channel.edit(topic=f"Server IP: {self.ip}")
        except discord.HTTPException:
            pass  # topic is a nice-to-have
        await interaction.response.edit_message(content=f"Posted in **#{channel.name}** ✅", view=None)


class IPView(discord.ui.View):
    def __init__(self, guild: discord.Guild, ip: str):
        super().__init__(timeout=120)
        self.add_item(IPChannelSelect(guild, ip))


async def find_allowed_guild(user: discord.User):
    """First server where this user is an admin (or listed in ALLOWED_USER_IDS)."""
    for guild in bot.guilds:
        try:
            member = guild.get_member(user.id) or await guild.fetch_member(user.id)
        except discord.NotFound:
            continue
        if member.guild_permissions.administrator or user.id in ALLOWED_IDS:
            return guild
    return None


@bot.event
async def on_message(message: discord.Message):
    # Only react to DMs from real people
    if message.author.bot or message.guild is not None:
        return

    ip = message.content.strip().strip("`")
    ip = re.sub(r"^ip\s*[:=]?\s+", "", ip, flags=re.IGNORECASE).strip()

    if not ip or len(ip) > 100 or any(ch.isspace() for ch in ip):
        await message.channel.send("DM me just the server IP, like `play.example.com:25565` 💖")
        return

    guild = await find_allowed_guild(message.author)
    if guild is None:
        await message.channel.send("Sorry, only server admins can do that 🙈")
        return

    await message.channel.send(f"Got it! Where should I post `{ip}`?", view=IPView(guild, ip))


# ------------------------- welcome page: friend roles and special roles -------------------------
# name, colour, emoji, label, short description. Add or remove lines to change the choices (max 25 per menu).
ROLE_GROUPS = {
    "friend": {
        "title": "💞 Friend roles",
        "placeholder": "How do we know you?",
        "roles": [
            ("💖 Bestie", "FF8FCB", "💖", "Bestie", "My closest people"),
            ("🤝 Friend", "A78BFA", "🤝", "Friend", "A friend of the server"),
            ("🏡 Family", "F4C27A", "🏡", "Family", "Family"),
            ("👋 New Here", "9CA3AF", "👋", "New Here", "Just arrived, say hi!"),
        ],
    },
    "special": {
        "title": "✨ Special roles",
        "placeholder": "What are you great at?",
        "roles": [
            ("🧱 Builder", "C28B5B", "🧱", "Builder", "Builds big and pretty"),
            ("🔮 Magician", "8B5CF6", "🔮", "Magician", "Magic mods and spells"),
            ("⚔️ Adventurer", "EF4444", "⚔️", "Adventurer", "Dungeons, bosses and loot"),
            ("🧪 Redstoner", "DC2626", "🧪", "Redstoner", "Machines and contraptions"),
            ("🌾 Farmer", "84CC16", "🌾", "Farmer", "Crops, animals and cosy vibes"),
            ("🎨 Artist", "F472B6", "🎨", "Artist", "Pixel art and decorating"),
            ("🕵️ Explorer", "38BDF8", "🕵️", "Explorer", "Finds every biome"),
            ("🎵 Musician", "FBBF24", "🎵", "Musician", "Note blocks and good tunes"),
        ],
    },
}


class GroupSelect(discord.ui.Select):
    def __init__(self, key: str):
        g = ROLE_GROUPS[key]
        options = [
            discord.SelectOption(label=label, value=name, emoji=emoji, description=desc)
            for name, _c, emoji, label, desc in g["roles"]
        ]
        super().__init__(placeholder=g["placeholder"], min_values=0, max_values=len(options),
                         options=options, custom_id=f"welcome:{key}")
        self.key = key

    async def callback(self, interaction: discord.Interaction):
        member = interaction.user
        want = set(self.values)
        added, removed, missing = [], [], []
        try:
            for name, _c, emoji, label, _d in ROLE_GROUPS[self.key]["roles"]:
                role = discord.utils.get(interaction.guild.roles, name=name)
                if role is None:
                    if name in want:
                        missing.append(label)
                    continue
                if name in want and role not in member.roles:
                    await member.add_roles(role)
                    added.append(f"{emoji} {label}")
                elif name not in want and role in member.roles:
                    await member.remove_roles(role)
                    removed.append(f"{emoji} {label}")
        except discord.Forbidden:
            await interaction.response.send_message(
                "I can't change that role. Ask an admin to move my role above the others.", ephemeral=True)
            return
        parts = []
        if added:
            parts.append("**Added:** " + ", ".join(added))
        if removed:
            parts.append("**Removed:** " + ", ".join(removed))
        if missing:
            parts.append("Not set up yet: " + ", ".join(missing) + ". Ask an admin to run /welcome.")
        if not parts:
            parts.append("No changes. Pick the ones that fit you, and the menu keeps your list up to date.")
        await interaction.response.send_message(embed=fancy("Your roles ✨", "\n".join(parts), PINK), ephemeral=True)


class WelcomeView(discord.ui.View):
    def __init__(self):
        super().__init__(timeout=None)
        for key in ROLE_GROUPS:
            self.add_item(GroupSelect(key))


def welcome_embed(guild: discord.Guild, text: Optional[str]) -> discord.Embed:
    body = text or ("Glad you're here! 💖\n\nPick your roles with the menus below so everyone knows who you are. "
                    "You can change them any time. The menu always sets your roles to exactly what you pick.")
    e = fancy(f"✦ ᴡᴇʟᴄᴏᴍᴇ ᴛᴏ {guild.name} ✦", body, PINK)
    for g in ROLE_GROUPS.values():
        e.add_field(name=g["title"], value="\n".join(f"{emoji} **{label}**: {desc}" for _n, _c, emoji, label, desc in g["roles"]), inline=False)
    if guild.icon:
        e.set_thumbnail(url=guild.icon.url)
    return e


@bot.tree.command(name="welcome", description="Post the welcome page with friend roles and special roles")
@app_commands.describe(channel="Where to post it (default: this channel)", message="Your own welcome text (optional)")
@app_commands.default_permissions(administrator=True)
@app_commands.guild_only()
async def welcome(interaction: discord.Interaction, channel: Optional[discord.TextChannel] = None, message: Optional[str] = None):
    await interaction.response.defer(ephemeral=True)
    guild = interaction.guild
    made = []
    try:
        for g in ROLE_GROUPS.values():
            for name, colour, _e, _l, _d in g["roles"]:
                if not discord.utils.get(guild.roles, name=name):
                    await guild.create_role(name=name, colour=discord.Colour(int(colour, 16)), mentionable=True)
                    made.append(name)
        target = channel or interaction.channel
        await target.send(embed=welcome_embed(guild, message), view=WelcomeView())
    except discord.Forbidden:
        await interaction.followup.send("I'm missing a permission (Manage Roles, or Send Messages in that channel).", ephemeral=True)
        return
    note = f"Posted in {target.mention}."
    if made:
        note += "\nCreated roles: " + ", ".join(made)
    await interaction.followup.send(embed=fancy("Welcome page ready ✨", note, PINK), ephemeral=True)


# ------------------------- mod list (kept up to date by the panel) -------------------------
# The panel writes modlist.json next to this file every couple of minutes.
MODLIST_FILE = "modlist.json"
MODPOSTS_FILE = "modlist_posts.json"


def load_json(path, default):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def save_json(path, data):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f)
    os.replace(tmp, path)


def mod_pages(server: dict, data: dict):
    mods = sorted(server.get("mods") or [], key=lambda m: str(m.get("n", "")).lower())
    lines = []
    for m in mods:
        name = str(m.get("n", "?")).replace("[", "(").replace("]", ")")
        ver = f" `{m['v']}`" if m.get("v") else ""
        url = str(m.get("u") or "")
        lines.append(f"• [{name}]({url}){ver}" if url.startswith("https://") else f"• **{name}**{ver}")
    pages, cur = [], ""
    for ln in lines:
        if len(cur) + len(ln) + 1 > 3800:
            pages.append(cur)
            cur = ""
        cur += ln + "\n"
    if cur:
        pages.append(cur)
    if not pages:
        pages = ["No mods installed yet."]
    out = []
    for i, text in enumerate(pages):
        e = discord.Embed(description=text, colour=GREEN)
        if i == 0:
            e.title = f"✦ Mods on {server.get('name', 'the server')} ✦"
        e.set_footer(text=f"{len(mods)} mods" + (f" · page {i + 1}/{len(pages)}" if len(pages) > 1 else "") + " · updates itself")
        try:
            e.timestamp = datetime.fromtimestamp(float(data.get("updated") or 0), timezone.utc)
        except (ValueError, OSError, OverflowError):
            pass
        out.append(e)
    return out


def digest(embeds) -> str:
    return hashlib.sha1(json.dumps([e.to_dict() for e in embeds], sort_keys=True, default=str).encode()).hexdigest()


async def server_ac(interaction: discord.Interaction, current: str):
    names = [s.get("name", "") for s in (load_json(MODLIST_FILE, {}).get("servers") or [])]
    return [app_commands.Choice(name=n[:100], value=n[:100]) for n in names if current.lower() in n.lower()][:25]


@bot.tree.command(name="modlist", description="Post the mod list with links (it keeps itself up to date)")
@app_commands.describe(server="Which server, if you run more than one")
@app_commands.autocomplete(server=server_ac)
@app_commands.default_permissions(administrator=True)
@app_commands.guild_only()
async def modlist(interaction: discord.Interaction, server: Optional[str] = None):
    data = load_json(MODLIST_FILE, {})
    servers = data.get("servers") or []
    if not servers:
        await interaction.response.send_message("I don't have a mod list yet. Make sure the panel is running, wait a minute, and try again.", ephemeral=True)
        return
    srv = next((s for s in servers if server and s.get("name", "").lower() == server.lower()), None) if server else servers[0]
    if srv is None:
        await interaction.response.send_message("I couldn't find that server.", ephemeral=True)
        return
    await interaction.response.defer(ephemeral=True)
    embeds = mod_pages(srv, data)
    ids = []
    try:
        for e in embeds:
            ids.append((await interaction.channel.send(embed=e)).id)
    except discord.Forbidden:
        await interaction.followup.send("I can't post in this channel. Give me Send Messages and Embed Links here.", ephemeral=True)
        return
    posts = load_json(MODPOSTS_FILE, [])
    posts.append({"channel": interaction.channel.id, "server": srv.get("id"), "messages": ids, "hash": digest(embeds)})
    save_json(MODPOSTS_FILE, posts[-20:])
    await interaction.followup.send(embed=fancy("Mod list posted ✨", "It updates itself whenever your mods change.", GREEN), ephemeral=True)


async def refresh_modlists():
    data = load_json(MODLIST_FILE, {})
    servers = {s.get("id"): s for s in (data.get("servers") or [])}
    posts = load_json(MODPOSTS_FILE, [])
    keep, changed = [], False
    for p in posts:
        srv = servers.get(p.get("server"))
        if srv is None:
            keep.append(p)
            continue
        embeds = mod_pages(srv, data)
        h = digest(embeds)
        if h == p.get("hash"):
            keep.append(p)
            continue
        try:
            ch = bot.get_channel(p["channel"]) or await bot.fetch_channel(p["channel"])
            ids = list(p.get("messages") or [])
            for i, e in enumerate(embeds):
                if i < len(ids):
                    await ch.get_partial_message(ids[i]).edit(embed=e)
                else:
                    ids.append((await ch.send(embed=e)).id)
            for mid in ids[len(embeds):]:
                try:
                    await ch.get_partial_message(mid).delete()
                except discord.HTTPException:
                    pass
            p["messages"], p["hash"] = ids[:len(embeds)], h
            keep.append(p)
            changed = True
        except discord.NotFound:
            changed = True  # the channel or message was deleted, stop tracking it
        except discord.HTTPException:
            keep.append(p)
    if changed:
        save_json(MODPOSTS_FILE, keep)


async def modlist_loop():
    await bot.wait_until_ready()
    while not bot.is_closed():
        try:
            await refresh_modlists()
            await refresh_fingerprints()
        except Exception as ex:
            print(f"mod list refresh problem: {ex}")
        await asyncio.sleep(120)


# ------------------------- AutoModpack fingerprint -------------------------
# The panel reads the fingerprint from the server's log and keeps it in modlist.json.
FPPOSTS_FILE = "fingerprint_posts.json"


def fp_embed(server: dict) -> discord.Embed:
    name = server.get("name", "the server")
    e = fancy(
        "✦ ᴀᴜᴛᴏᴍᴏᴅᴘᴀᴄᴋ ꜰɪɴɢᴇʀᴘʀɪɴᴛ ✦",
        f"The first time you join **{name}**, AutoModpack asks you to check the server's fingerprint. "
        f"Make sure it matches this one, then accept:\n```\n{server.get('fp', '')}\n```",
        GREEN,
    )
    e.set_footer(text="Only trust a fingerprint that comes from the server owner · updates itself")
    return e


@bot.tree.command(name="fingerprint", description="Post the AutoModpack fingerprint so people can copy it")
@app_commands.describe(server="Which server, if you run more than one")
@app_commands.autocomplete(server=server_ac)
@app_commands.default_permissions(administrator=True)
@app_commands.guild_only()
async def fingerprint(interaction: discord.Interaction, server: Optional[str] = None):
    servers = load_json(MODLIST_FILE, {}).get("servers") or []
    if server:
        srv = next((s for s in servers if s.get("name", "").lower() == server.lower()), None)
    else:
        srv = next((s for s in servers if s.get("fp")), servers[0] if servers else None)
    if srv is None:
        await interaction.response.send_message("I don't know that server yet. Make sure the panel is running, wait a minute, and try again.", ephemeral=True)
        return
    if not srv.get("fp"):
        await interaction.response.send_message(
            f"I haven't seen an AutoModpack fingerprint for **{srv.get('name')}** yet. Start the server once with AutoModpack installed, wait a minute, and try again.",
            ephemeral=True)
        return
    try:
        msg = await interaction.channel.send(embed=fp_embed(srv))
    except discord.Forbidden:
        await interaction.response.send_message("I can't post in this channel. Give me Send Messages and Embed Links here.", ephemeral=True)
        return
    posts = load_json(FPPOSTS_FILE, [])
    posts.append({"channel": interaction.channel.id, "message": msg.id, "server": srv.get("id"), "fp": srv.get("fp")})
    save_json(FPPOSTS_FILE, posts[-20:])
    await interaction.response.send_message(embed=fancy("Fingerprint posted ✨", "If the fingerprint ever changes, I'll edit that message.", GREEN), ephemeral=True)


async def refresh_fingerprints():
    servers = {s.get("id"): s for s in (load_json(MODLIST_FILE, {}).get("servers") or [])}
    posts = load_json(FPPOSTS_FILE, [])
    keep, changed = [], False
    for p in posts:
        srv = servers.get(p.get("server"))
        if srv is None or not srv.get("fp") or srv.get("fp") == p.get("fp"):
            keep.append(p)
            continue
        try:
            ch = bot.get_channel(p["channel"]) or await bot.fetch_channel(p["channel"])
            await ch.get_partial_message(p["message"]).edit(embed=fp_embed(srv))
            p["fp"] = srv["fp"]
            keep.append(p)
            changed = True
        except discord.NotFound:
            changed = True
        except discord.HTTPException:
            keep.append(p)
    if changed:
        save_json(FPPOSTS_FILE, keep)


# ---------------------------------------------------------------
# /start: admins can switch a Minecraft server ON from Discord.
# There is deliberately no /stop. The bot talks to the panel with its own
# key, which the panel only accepts for "list servers" and "start a server".
# ---------------------------------------------------------------
HUB_PORT = os.getenv("HUB_PORT", "")
BOT_KEY = os.getenv("BOT_KEY", "")
START_COOLDOWN = 45          # seconds between /start presses for the same server
_last_start = {}


def hub_call(method: str, path: str, body=None, timeout: int = 30):
    if not HUB_PORT or not BOT_KEY:
        return None
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"http://127.0.0.1:{HUB_PORT}{path}", data=data, method=method,
        headers={"X-Key": BOT_KEY, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"ok": False, "msg": f"The panel answered with an error ({e.code})."}
    except Exception:
        return None


STATE_LABEL = {"online": "online", "starting": "starting", "stopping": "stopping", "crashed": "crashed"}


async def start_ac(interaction: discord.Interaction, current: str):
    data = await asyncio.to_thread(hub_call, "GET", "/api/hub/bot/servers", None, 4)
    out = []
    for sv in (data or {}).get("servers") or []:
        name = str(sv.get("name", ""))
        if current.lower() not in name.lower():
            continue
        label = STATE_LABEL.get(sv.get("state"), "offline")
        out.append(app_commands.Choice(name=f"{name} ({label})"[:100], value=str(sv.get("id", ""))[:100]))
    return out[:25]


@bot.tree.command(name="start", description="Start one of the Minecraft servers (admins only)")
@app_commands.describe(server="Which server to start")
@app_commands.autocomplete(server=start_ac)
@app_commands.default_permissions(administrator=True)
@app_commands.guild_only()
async def start_server(interaction: discord.Interaction, server: str):
    # Discord's own permission setting can be changed by the server owner, so check again here.
    if not interaction.permissions.administrator:
        await interaction.response.send_message("Only server admins can start a server.", ephemeral=True)
        return
    await interaction.response.defer(thinking=True)
    data = await asyncio.to_thread(hub_call, "GET", "/api/hub/bot/servers", None, 8)
    if data is None:
        await interaction.edit_original_response(embed=fancy("Can't reach the panel", "The panel on the host computer isn't answering. Is the Mac awake?", PURPLE))
        return
    if data.get("enabled") is False:
        await interaction.edit_original_response(embed=fancy("/start is switched off", "An admin turned this command off in the panel's Discord window.", PURPLE))
        return
    servers = data.get("servers") or []
    want = server.strip().lower()
    sv = next((x for x in servers if x.get("id") == server), None) or next((x for x in servers if str(x.get("name", "")).lower() == want), None)
    if sv is None:
        await interaction.edit_original_response(embed=fancy("No server by that name", "Pick one from the list that appears when you type /start.", PURPLE))
        return
    name, sid = str(sv.get("name")), str(sv.get("id"))
    if sv.get("state") in ("online", "starting"):
        word = "already online" if sv.get("state") == "online" else "already starting up"
        await interaction.edit_original_response(embed=fancy(f"{name} is {word}", "", GREEN))
        return
    wait = START_COOLDOWN - (time.time() - _last_start.get(sid, 0))
    if wait > 0:
        await interaction.edit_original_response(embed=fancy("Hold on a moment", f"Someone just pressed start for {name}. Try again in {int(wait) + 1}s.", PURPLE))
        return
    _last_start[sid] = time.time()
    res = await asyncio.to_thread(hub_call, "POST", "/api/hub/bot/start", {"id": sid}, 30)
    if not res or not res.get("ok"):
        _last_start.pop(sid, None)
        why = (res or {}).get("msg") or "The panel didn't answer."
        await interaction.edit_original_response(embed=fancy(f"{name} didn't start", str(why)[:1000], PURPLE))
        return
    await interaction.edit_original_response(embed=fancy(f"Starting {name}...", f"Started by {interaction.user.mention}. I'll update this message when it's ready (a modpack can take a few minutes).", ORANGE))
    log(f"/start {name} by {interaction.user} ({interaction.user.id}) in {interaction.guild}")
    for _ in range(60):                       # up to 5 minutes
        await asyncio.sleep(5)
        now = await asyncio.to_thread(hub_call, "GET", "/api/hub/bot/servers", None, 6)
        cur = next((x for x in (now or {}).get("servers") or [] if x.get("id") == sid), None)
        if cur and cur.get("state") == "online":
            await interaction.edit_original_response(embed=fancy(f"{name} is online", "Hop on in!", GREEN))
            return
        if cur and cur.get("state") in ("stopped", "crashed"):
            await interaction.edit_original_response(embed=fancy(f"{name} stopped again", "It didn't stay up. Check its console in the panel.", PURPLE))
            return
    await interaction.edit_original_response(embed=fancy(f"{name} is still starting", "It's taking a while. Check back in a minute.", ORANGE))


def log(msg: str):
    print(f"[{datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M:%S')}] {msg}", flush=True)


def apply_features():
    """The panel's Discord window can switch /modlist, /welcome, /fingerprint and /start off (takes effect when the bot restarts)."""
    f = load_json("features.json", {})
    for name, cmd in (("modlist", "modlist"), ("welcome", "welcome"), ("fingerprint", "fingerprint"), ("startcmd", "start")):
        if f.get(name) is False:
            bot.tree.remove_command(cmd)


apply_features()


if __name__ == "__main__":
    if not TOKEN:
        raise SystemExit("Missing DISCORD_TOKEN. Put it in a .env file (see README).")
    bot.run(TOKEN)
