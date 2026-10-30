import {useState} from 'react';
import {ArrowUpRight, Check, ArrowLeft} from 'lucide-react';
import {Button} from './components/ui/button';
import './icon-review.css';

const directions=[
 {id:'a-dialogue',label:'A',name:'双向对话',tag:'推荐',description:'两段对话，连成一个网络。直接表达 Agent 之间的交流。'},
 {id:'b-nexus',label:'B',name:'连接星',tag:'更抽象',description:'连接向四面展开。强调开放网络与未来感。'},
 {id:'c-companion',label:'C',name:'通讯伙伴',tag:'更亲切',description:'Agent 的脸，也是一个对话气泡。更适合发展成品牌角色。'},
];
export default function IconReview(){
 const [selected,setSelected]=useState('a-dialogue');const current=directions.find(d=>d.id===selected);
 return <main className="brand-review wrap">
  <header><a href="/"><ArrowLeft size={16}/> 回到 AIConnect</a><span>BRAND / 01</span></header>
  <div className="review-heading"><p className="eyebrow">AICONNECT · ICON DIRECTIONS</p><h1>为下一条连接，<br/><span>选一个记号。</span></h1><p>同一抹电光绿。三种不同的性格。</p></div>
  <div className="direction-grid">{directions.map(d=><Button variant="ghost" key={d.id} aria-pressed={selected===d.id} className={`direction-card ${selected===d.id?'selected':''}`} onClick={()=>setSelected(d.id)}><span className="direction-top"><span>{d.label} / {d.name}</span><span>{d.tag}</span></span><span className="direction-art"><img src={`/brand/${d.id}.svg`} alt={`${d.name}图标`}/></span><span className="direction-description">{d.description}</span><span className="direction-pick">{selected===d.id?<><Check size={16}/> 正在预览</>:<>预览这个方向 <ArrowUpRight size={16}/></>}</span></Button>)}</div>
  <section className="brand-context" aria-label="选中图标的应用预览"><div className="context-title"><span>{current.label} / {current.name}</span><span>APPLICATION PREVIEW</span></div><div className="context-grid"><div className="wordmark-preview"><img src={`/brand/${selected}.svg`} alt=""/><span>AIConnect<span className="brand-period">.</span></span></div><div className="app-icon-preview"><span className="app-tile"><img src={`/brand/${selected}.svg`} alt="应用图标"/></span><span className="mini-icon"><img src={`/brand/${selected}.svg`} alt="小尺寸图标"/> AIConnect</span></div><div className="light-icon-preview"><img src={`/brand/${selected}.svg`} alt="单色图标"/><span>单色也成立。</span></div></div></section>
  <footer><p>点击查看应用效果，告诉我你选 A、B 还是 C。确认后再替换正式图标。</p><Button variant="outline" asChild><a href="/">查看首页 <ArrowUpRight size={15}/></a></Button></footer>
 </main>
}
